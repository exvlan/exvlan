//! `era.exvlan.studio.helper` 的 GUI 侧封装（仅 macOS）。
//!
//! 首次安装由一次提权完成（同一次里装内核 + 服务 + helper）；之后的 App 升级
//! 只需要把「已签名的内核」交给常驻 helper，**不再弹授权**。

#![allow(dead_code)]

/// 必须与 `packages/studio/helper` 的 `HELPER_VERSION` 保持一致。
///
/// pre.11 从 2 → 3：新增「socket 未就绪时重试」，且 apply 前先 verify_healthy。
/// 提升版本号是为了强制走一次完整重装——否则 `apply` 用的是旧 helper 二进制，
/// 新逻辑要等旧二进制完成自更新后才生效（首次升级仍会踩 EOF）。
pub const HELPER_VERSION: u32 = 3;

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use anyhow::{Context as _, bail};
    use std::path::{Path, PathBuf};
    use tauri::Manager as _;

    const HELPER_LABEL: &str = "era.exvlan.studio.helper";

    fn helper_bin() -> PathBuf {
        crate::paths::helper_bin().expect("helper 仅 macOS 存在")
    }

    fn helper_plist_path() -> PathBuf {
        crate::paths::helper_plist().expect("helper 仅 macOS 存在")
    }

    fn helper_version_file() -> PathBuf {
        crate::paths::helper_version_file().expect("helper 仅 macOS 存在")
    }

    /// 已安装且版本匹配。
    pub fn is_installed() -> bool {
        if !helper_bin().is_file() {
            return false;
        }
        let text = std::fs::read_to_string(helper_version_file()).unwrap_or_default();
        text.split_whitespace()
            .next()
            .and_then(|v| v.parse::<u32>().ok())
            == Some(super::HELPER_VERSION)
    }

    /// 已安装 **且进程真的在跑**（socket 可用）。
    ///
    /// `is_installed()` 只看文件+版本号，历史上出现过「版本文件写了、helper 从没起来」
    /// 的情况，导致之后永远不再修复。启动判断应使用本函数。
    pub fn is_ready() -> bool {
        if !is_installed() {
            return false;
        }
        matches!(status(), Ok(s) if !s.is_empty() && s != "not installed")
    }

    /// 随包 helper 的路径。
    pub fn bundled_helper(app: &tauri::AppHandle) -> Option<PathBuf> {
        let dir = app.path().resource_dir().ok()?;
        let candidates = [
            dir.join("helper").join("exvlan-helper"),
            dir.join("resources").join("helper").join("exvlan-helper"),
            dir.join("exvlan-helper"),
        ];
        candidates.into_iter().find(|p| p.is_file())
    }

    /// 安装 helper（由提权子进程以 root 调用；`src` 是随包 helper 的绝对路径）。
    ///
    /// 直接复制 + 写 plist + bootstrap（不执行随包二进制，避免 quarantine 干扰），
    /// 与 ClashBar / Clash Verge 的安装脚本一致。
    pub fn install_from(src: &Path, owner_uid: u32) -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt as _;

        if !src.is_file() {
            bail!("随包 helper 不存在: {}", src.display());
        }
        if let Some(parent) = helper_bin().parent() {
            std::fs::create_dir_all(parent).ok();
        }
        if let Some(parent) = helper_plist_path().parent() {
            std::fs::create_dir_all(parent).ok();
        }
        // helper 日志目录（系统日志根）
        std::fs::create_dir_all(crate::paths::system().logs).ok();

        let bin = helper_bin();
        let plist = helper_plist_path();
        std::fs::copy(src, &bin).with_context(|| format!("复制 helper 到 {}", bin.display()))?;
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755))?;
        strip_quarantine(&bin);

        std::fs::write(&plist, helper_plist(&bin, &crate::paths::helper_log()))
            .with_context(|| format!("写入 {}", plist.display()))?;
        std::fs::set_permissions(&plist, std::fs::Permissions::from_mode(0o644))?;
        strip_quarantine(&plist);

        // 清理历史 label / 残留 plist：新旧 helper 并存会导致端口/socket 争用，
        // 且旧 plist 会让 launchctl 找到"另一个同名服务的旧定义"。
        for legacy in [
            "era.exvlan.studio.helper",
            "exvlan-studio.helper",
            "com.exvlan.studio.helper",
        ] {
            let _ = launchctl(&["bootout", &format!("system/{legacy}")]);
            let _ = std::fs::remove_file(
                crate::paths::launch_daemons_dir().join(format!("{legacy}.plist")),
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
        let _ = launchctl(&["bootout", &format!("system/{HELPER_LABEL}")]);
        std::thread::sleep(std::time::Duration::from_millis(300));

        // launchd 的注销是异步的：bootout 之后立刻 bootstrap 常见
        // `Bootstrap failed: 5: Input/output error`。旧代码把这个错误吞掉，
        // 于是「装上了但没在跑」还会写版本文件 → 之后永远不再修复。
        let mut last_err = String::new();
        let mut booted = false;
        for attempt in 1..=3u32 {
            match launchctl(&["bootstrap", "system", &plist.to_string_lossy()]) {
                Ok(out) if out.status.success() => {
                    booted = true;
                    break;
                }
                Ok(out) => {
                    last_err = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    if last_err.contains("already bootstrapped") || last_err.contains("already") {
                        booted = true;
                        break;
                    }
                    crate::logger::record(
                        "helper.install",
                        format!("bootstrap attempt={attempt}/3 failed: {last_err}"),
                    );
                    let _ = launchctl(&["bootout", &format!("system/{HELPER_LABEL}")]);
                    std::thread::sleep(std::time::Duration::from_millis(300 * u64::from(attempt)));
                }
                Err(err) => {
                    last_err = err.to_string();
                    std::thread::sleep(std::time::Duration::from_millis(300));
                }
            }
        }
        if !booted {
            bail!(
                "launchctl bootstrap 失败（3 次）：{last_err}\n{}",
                diag_snapshot()
            );
        }

        let _ = launchctl(&["kickstart", "-k", &format!("system/{HELPER_LABEL}")]);

        // 健康检查：socket 真的能连才算装好；否则**不写版本文件**，
        // 让 ensure_kernel_service 下次仍判定 need_helper，自动重试。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut last = String::new();
        while std::time::Instant::now() < deadline {
            match status() {
                Ok(s) if !s.is_empty() && s != "not installed" => {
                    let version_file = helper_version_file();
                    std::fs::write(&version_file, format!("{HELPER_VERSION} {owner_uid}\n"))?;
                    std::fs::set_permissions(
                        &version_file,
                        std::fs::Permissions::from_mode(0o644),
                    )?;
                    return Ok(());
                }
                Ok(s) => last = s,
                Err(e) => last = e,
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }

        bail!(
            "helper 已安装但未就绪（socket 不可用）：{last}\n{}",
            diag_snapshot()
        )
    }

    fn launchctl(args: &[&str]) -> std::io::Result<std::process::Output> {
        std::process::Command::new("/bin/launchctl")
            .args(args)
            .output()
    }

    /// 安装/启动失败时的现场（launchctl 状态 + helper 日志尾）。
    fn diag_snapshot() -> String {
        let print = std::process::Command::new("/bin/launchctl")
            .args(["print", &format!("system/{HELPER_LABEL}")])
            .output()
            .map(|o| {
                let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
                if text.len() > 1200 {
                    text.truncate(1200);
                }
                text.replace('\n', " | ")
            })
            .unwrap_or_default();
        format!("launchctl: {print} | log tail: {}", helper_log_tail(2048))
    }

    fn strip_quarantine(path: &Path) {
        let _ = std::process::Command::new("xattr")
            .args(["-d", "com.apple.quarantine"])
            .arg(path)
            .output();
    }

    fn helper_plist(bin: &Path, log: &Path) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{HELPER_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{bin}</string>
    <string>--daemon</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ProcessType</key><string>Interactive</string>
  <key>ThrottleInterval</key><integer>10</integer>
  <key>StandardOutPath</key><string>{log}</string>
  <key>StandardErrorPath</key><string>{log}</string>
</dict>
</plist>
"#,
            bin = bin.display(),
            log = log.display(),
        )
    }

    /// 把随包的已签名内核交给 helper（当前用户执行，无需提权）。
    pub fn apply_bundled(app: &tauri::AppHandle) -> anyhow::Result<()> {
        if !is_installed() {
            bail!("helper 未安装");
        }
        let kernel = crate::kernel::bundled_kernel(app).context("未找到随包内核")?;
        let sig = kernel.with_extension("sig");
        if !sig.is_file() {
            bail!("缺少内核签名 {}", sig.display());
        }

        let staging = std::env::temp_dir().join(format!(
            "era-exvlan-update-{}-{}",
            std::process::id(),
            crate::logger::timestamp().replace(':', "-")
        ));
        std::fs::create_dir_all(&staging)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o700))?;
        }
        std::fs::copy(&kernel, staging.join("kernel"))?;
        std::fs::copy(&sig, staging.join("kernel.sig"))?;
        std::fs::write(staging.join("version"), crate::BUILD_VERSION)?;

        // helper 自更新：升级包内带新 helper + 签名时一并交给 root 侧替换，
        // 这样连 helper 版本升级也不需要再提权。
        if let Some(helper) = bundled_helper(app) {
            let helper_sig = helper.with_extension("sig");
            if helper_sig.is_file() {
                std::fs::copy(&helper, staging.join("helper"))?;
                std::fs::copy(&helper_sig, staging.join("helper.sig"))?;
                std::fs::write(staging.join("helper_version"), HELPER_VERSION.to_string())?;
            }
        }

        // helper 可能刚被 launchd 拉起：先确认 socket 能应答，再提交 apply，
        // 否则会拿到 EOF 并误判为「helper 不可用」而回退提权。
        verify_healthy(std::time::Duration::from_secs(5));

        let out = std::process::Command::new(helper_bin())
            .arg("--apply")
            .arg(&staging)
            .output()
            .with_context(|| format!("执行 {} --apply 失败", helper_bin().display()))?;
        let _ = std::fs::remove_dir_all(&staging);
        if !out.status.success() {
            bail!(
                "helper apply 失败: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }

    fn socket_request(op: &str, extra: serde_json::Value) -> Result<String, String> {
        // helper 可能刚被 launchd 拉起，socket 还未就绪；只对「连不上」重试，
        // 请求已送达但响应异常时不重试（避免 `spawn` 被重复执行）。
        let mut last = String::new();
        for attempt in 0..5u64 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(200 * attempt));
            }
            match socket_request_once(op, &extra) {
                Ok(value) => return Ok(value),
                Err(err) if err.contains("连接 helper socket 失败") => last = err,
                Err(err) => return Err(err),
            }
        }
        Err(last)
    }

    fn socket_request_once(op: &str, extra: &serde_json::Value) -> Result<String, String> {
        use std::io::{BufRead as _, BufReader, Write as _};
        let mut stream = std::os::unix::net::UnixStream::connect(crate::paths::helper_socket())
            .map_err(|e| format!("连接 helper socket 失败: {e}"))?;
        let mut body = serde_json::json!({ "op": op });
        if let (Some(obj), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
            for (k, v) in extra {
                obj.insert(k.clone(), v.clone());
            }
        }
        writeln!(stream, "{body}").map_err(|e| format!("发送请求失败: {e}"))?;
        let mut line = String::new();
        BufReader::new(stream)
            .read_line(&mut line)
            .map_err(|e| format!("读取响应失败: {e}"))?;
        let resp: serde_json::Value =
            serde_json::from_str(line.trim()).map_err(|e| format!("解析响应失败: {e}"))?;
        if resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
            Ok(resp
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string())
        } else {
            Err(resp
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("helper 返回失败")
                .to_string())
        }
    }

    /// 让 root helper 以**普通模式**启动内核，并随 GUI（`parent_pid`）退出。
    pub fn spawn_kernel(args: &[String], parent_pid: u32) -> Result<String, String> {
        socket_request(
            "spawn",
            serde_json::json!({ "args": args, "parent_pid": parent_pid }),
        )
    }

    /// 停止由 helper 托管的内核。
    pub fn kill_kernel() -> Result<String, String> {
        socket_request("kill", serde_json::json!({}))
    }

    /// 安装后健康检查：轮询 `--status`（socket 连通即视为健康）。
    ///
    /// 刚 bootstrap 完的几百毫秒内 socket 还没 bind，`--status` 会报
    /// `Connection refused`；这只算「还没起来」，不能当成安装失败。
    /// 真正超时才把 `launchctl print` 与 helper 日志尾部写进 error.log。
    pub fn verify_healthy(timeout: std::time::Duration) {
        let started = std::time::Instant::now();
        let mut last = String::new();
        while started.elapsed() < timeout {
            match status() {
                Ok(s) => {
                    crate::logger::record(
                        "helper.health",
                        format!("ok in {}ms: {s}", started.elapsed().as_millis()),
                    );
                    return;
                }
                Err(e) => last = e,
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }

        let print = std::process::Command::new("/bin/launchctl")
            .args(["print", &format!("system/{HELPER_LABEL}")])
            .output()
            .map(|o| {
                let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
                if text.len() > 1500 {
                    text.truncate(1500);
                }
                text
            })
            .unwrap_or_default();
        crate::logger::record(
            "helper.health",
            format!(
                "failed after {}ms: {last} | launchctl: {print} | log tail: {}",
                timeout.as_millis(),
                helper_log_tail(4096)
            ),
        );
    }

    /// helper 日志尾部（仅用于健康检查失败时落盘）。
    fn helper_log_tail(max: usize) -> String {
        let path = crate::paths::helper_log();
        let Ok(bytes) = std::fs::read(&path) else {
            return format!("(无法读取 {})", path.display());
        };
        let start = bytes.len().saturating_sub(max);
        String::from_utf8_lossy(&bytes[start..]).replace('\n', " | ")
    }

    /// 查询 helper 状态（诊断用）。
    pub fn status() -> Result<String, String> {
        if !is_installed() {
            return Ok("not installed".to_string());
        }
        let out = std::process::Command::new(helper_bin())
            .arg("--status")
            .output()
            .map_err(|e| format!("执行 helper --status 失败: {e}"))?;
        if !out.status.success() {
            return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::path::{Path, PathBuf};

    pub fn is_installed() -> bool {
        false
    }

    pub fn is_ready() -> bool {
        false
    }

    pub fn bundled_helper(_app: &tauri::AppHandle) -> Option<PathBuf> {
        None
    }

    pub fn install_from(_src: &Path, _owner_uid: u32) -> anyhow::Result<()> {
        anyhow::bail!("helper 仅支持 macOS")
    }

    pub fn apply_bundled(_app: &tauri::AppHandle) -> anyhow::Result<()> {
        anyhow::bail!("helper 仅支持 macOS")
    }

    pub fn status() -> Result<String, String> {
        Ok("unsupported".to_string())
    }

    pub fn verify_healthy(_timeout: std::time::Duration) {}

    pub fn spawn_kernel(_args: &[String], _parent_pid: u32) -> Result<String, String> {
        Err("helper 仅 macOS 支持".to_string())
    }

    pub fn kill_kernel() -> Result<String, String> {
        Ok("noop".to_string())
    }
}

#[allow(unused_imports)]
pub use imp::{
    apply_bundled, bundled_helper, install_from, is_installed, is_ready, kill_kernel, spawn_kernel,
    status, verify_healthy,
};

/// 当前进程 uid（供提权安装使用）。
pub fn current_uid() -> u32 {
    #[cfg(unix)]
    {
        unsafe { libc::getuid() }
    }
    #[cfg(not(unix))]
    {
        0
    }
}
