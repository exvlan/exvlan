//! `era.exvlan.studio.helper` 的 GUI 侧封装（仅 macOS）。
//!
//! 首次安装由一次提权完成（同一次里装内核 + 服务 + helper）；之后的 App 升级
//! 只需要把「已签名的内核」交给常驻 helper，**不再弹授权**。

#![allow(dead_code)]

/// 必须与 `packages/helper` 的 `HELPER_VERSION` 保持一致。
pub const HELPER_VERSION: u32 = 1;

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
        std::fs::create_dir_all("/Library/PrivilegedHelperTools").ok();
        std::fs::create_dir_all("/Library/LaunchDaemons").ok();
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

        let _ = std::process::Command::new("/bin/launchctl")
            .args(["bootout", &format!("system/{HELPER_LABEL}")])
            .output();
        std::thread::sleep(std::time::Duration::from_millis(300));

        let out = std::process::Command::new("/bin/launchctl")
            .arg("bootstrap")
            .arg("system")
            .arg(&plist)
            .output()
            .context("执行 launchctl bootstrap")?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stderr.contains("already") && !stderr.contains("Input/output error") {
                bail!("launchctl bootstrap 失败: {}", stderr.trim());
            }
        }
        let _ = std::process::Command::new("/bin/launchctl")
            .args(["kickstart", "-k", &format!("system/{HELPER_LABEL}")])
            .output();

        let version_file = helper_version_file();
        std::fs::write(&version_file, format!("{HELPER_VERSION} {owner_uid}\n"))?;
        std::fs::set_permissions(&version_file, std::fs::Permissions::from_mode(0o644))?;
        Ok(())
    }

    fn strip_quarantine(path: &Path) {
        let _ = std::process::Command::new("/usr/bin/xattr")
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
            crate::error_log::timestamp().replace(':', "-")
        ));
        std::fs::create_dir_all(&staging)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o700))?;
        }
        std::fs::copy(&kernel, staging.join("kernel"))?;
        std::fs::copy(&sig, staging.join("kernel.sig"))?;
        std::fs::write(staging.join("version"), env!("CARGO_PKG_VERSION"))?;

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
                    crate::error_log::record(
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
        crate::error_log::record(
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
}

#[allow(unused_imports)]
pub use imp::{apply_bundled, bundled_helper, install_from, is_installed, status, verify_healthy};

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
