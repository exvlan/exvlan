//! `exvlan-service` 客户端：GUI 与唯一常驻特权进程之间的 IPC。
//!
//! macOS / Windows：unix socket / 命名管道；Linux 不引入常驻服务
//! （普通模式走 setcap + PDEATHSIG，服务模式走 systemd）。
//!
//! 设计要点：
//! - 模式只是生命周期策略：`mode.set(service ↔ normal)` **不重启内核**；
//! - normal 模式靠**连接存活**：GUI 保持一条 session 连接，断开即停内核；
//! - 所有权：安装时记录 owner uid（mac peercred 校验）；
//! - 升级零弹窗：服务已装时，新二进制通过 IPC 交给服务自己替换（`service.update`）。

#![allow(dead_code)]

use std::path::PathBuf;

/// 服务 label（macOS plist / Windows 服务名）。
pub const SERVICE_LABEL: &str = crate::paths::PRIVILEGED_SERVICE_LABEL;

/// `foo.exe` → `foo.exe.sig`（`with_extension("sig")` 会得到 `foo.sig`，
/// 而 `tauri signer sign` / CI 生成的是 `<完整文件名>.sig`）。
pub fn sig_path(bin: &std::path::Path) -> PathBuf {
    let mut raw = bin.as_os_str().to_os_string();
    raw.push(".sig");
    PathBuf::from(raw)
}

pub fn current_uid() -> u32 {
    #[cfg(unix)]
    unsafe {
        libc::getuid() as u32
    }
    #[cfg(not(unix))]
    {
        0
    }
}

/// 随包的服务二进制（`resources/service/exvlan-service`）。
pub fn bundled_service(app: &tauri::AppHandle) -> Option<PathBuf> {
    use tauri::Manager as _;
    let dir = app.path().resource_dir().ok()?;
    let bin = if cfg!(windows) {
        "exvlan-service.exe"
    } else {
        "exvlan-service"
    };
    [
        dir.join("service").join(bin),
        dir.join("resources").join("service").join(bin),
        dir.join(bin),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

/// 随包的控制面工具目录（`resources/tools/`：cli / gateway / migrate）。
pub fn bundled_tools_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    use tauri::Manager as _;
    let dir = app.path().resource_dir().ok()?;
    [
        dir.join("tools"),
        dir.join("resources").join("tools"),
        dir.clone(),
    ]
    .into_iter()
    .find(|candidate| candidate.is_dir())
}

/// 已安装的服务二进制落点（必须与 `exvlan-service` 内部的 `paths()` 一致）。
pub fn installed_service_bin() -> PathBuf {
    crate::paths::privileged_service_bin()
}

/// 二进制旁边的 `service.version`（服务在 install / 自更新时写入，0644）。
pub fn version_file_path(bin: &std::path::Path) -> PathBuf {
    let mut raw = bin.as_os_str().to_os_string();
    raw.push(".version");
    PathBuf::from(raw)
}

/// 服务当前状态（供界面判断是否要显示横幅 / 诊断包）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ServiceState {
    pub installed: bool,
    pub ready: bool,
    pub mode: String,
    pub kernel_pid: u32,
    pub status: String,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub use imp::{
    apply_bundled, ensure_kernel, ensure_normal, ensure_service, install, installed_version,
    is_installed, is_ready, kernel_status, pair_start, port_cleanup, port_owner, restart_kernel,
    set_extra_whitelist, set_mode, state, stop, stop_kernel, update, whitelist_info,
};

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub use stub::{
    apply_bundled, ensure_kernel, ensure_normal, ensure_service, install, installed_version,
    is_installed, is_ready, kernel_status, pair_start, port_cleanup, port_owner, restart_kernel,
    set_extra_whitelist, set_mode, state, stop, stop_kernel, update, whitelist_info,
};

// ---------------------------------------------------------------------------
// macOS / Windows：IPC 实现
// ---------------------------------------------------------------------------

#[cfg(any(target_os = "macos", target_os = "windows"))]
mod imp {
    use super::*;
    use std::io::{BufRead as _, BufReader, Write};
    use std::sync::Mutex;

    /// 保持打开的一条 session（normal 模式的存活锚点）。
    struct Session {
        writer: Box<dyn Write + Send>,
        #[allow(dead_code)]
        reader: BufReader<Box<dyn std::io::Read + Send>>,
    }

    static SESSION: Mutex<Option<Session>> = Mutex::new(None);

    #[cfg(target_os = "macos")]
    fn connect() -> Result<Session, String> {
        let path = crate::paths::privileged_service_socket();
        let stream = std::os::unix::net::UnixStream::connect(&path)
            .map_err(|e| format!("连接 {} 失败: {e}", path.display()))?;
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(30)));
        let writer = Box::new(stream.try_clone().map_err(|e| e.to_string())?);
        let reader = BufReader::new(Box::new(stream) as Box<dyn std::io::Read + Send>);
        Ok(Session { writer, reader })
    }

    #[cfg(target_os = "windows")]
    fn connect() -> Result<Session, String> {
        use std::fs::OpenOptions;
        // 命名管道在 Rust 标准库里按文件打开即可（读写）。
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(r"\\.\pipe\era.exvlan.studio.service")
            .map_err(|e| format!("连接服务管道失败: {e}"))?;
        let writer = Box::new(file.try_clone().map_err(|e| e.to_string())?);
        let reader = BufReader::new(Box::new(file) as Box<dyn std::io::Read + Send>);
        Ok(Session { writer, reader })
    }

    fn request(op: &str, extra: serde_json::Value) -> Result<serde_json::Value, String> {
        let mut session = connect()?;
        let mut body = serde_json::json!({ "proto": 1, "op": op });
        if let (Some(obj), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
            for (k, v) in extra {
                obj.insert(k.clone(), v.clone());
            }
        }
        writeln!(session.writer, "{body}").map_err(|e| format!("发送请求失败: {e}"))?;
        session.writer.flush().ok();
        let mut line = String::new();
        session
            .reader
            .read_line(&mut line)
            .map_err(|e| format!("读取响应失败: {e}"))?;
        if line.trim().is_empty() {
            return Err("服务返回空响应".to_string());
        }
        let resp: serde_json::Value =
            serde_json::from_str(line.trim()).map_err(|e| format!("解析响应失败: {e}"))?;
        if resp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false) {
            Ok(resp)
        } else {
            let code = resp.get("code").and_then(|v| v.as_str()).unwrap_or("error");
            let msg = resp
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("服务返回失败");
            Err(format!("{code}: {msg}"))
        }
    }

    /// 建立（或复用）session；normal 模式的生命周期由它决定。
    fn ensure_session() -> Result<(), String> {
        let mut guard = SESSION.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_some() {
            return Ok(());
        }
        let mut session = connect()?;
        writeln!(session.writer, r#"{{"proto":1,"op":"hello"}}"#).map_err(|e| e.to_string())?;
        session.writer.flush().ok();
        let mut line = String::new();
        session
            .reader
            .read_line(&mut line)
            .map_err(|e| format!("服务未响应 hello: {e}"))?;
        *guard = Some(session);
        Ok(())
    }

    pub fn is_installed() -> bool {
        #[cfg(target_os = "macos")]
        {
            installed_service_bin().is_file() && crate::paths::privileged_service_plist().is_file()
        }
        #[cfg(target_os = "windows")]
        {
            installed_service_bin().is_file()
        }
    }

    /// 已安装服务的版本：优先读版本文件（零子进程、零窗口），
    /// 文件缺失时才回退到 `--version`（用 `hidden_command` 避免闪 cmd 窗口）。
    pub fn installed_version() -> Option<String> {
        let bin = installed_service_bin();
        if !bin.is_file() {
            return None;
        }
        if let Ok(text) = std::fs::read_to_string(version_file_path(&bin)) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
        let out = crate::kernel::hidden_command(&bin)
            .arg("--version")
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&out.stdout);
        // 形如 "exvlan-service 26.410.20712-pre.13+abcd"
        text.split_whitespace().nth(1).map(str::to_owned)
    }

    pub fn is_ready() -> bool {
        request("ping", serde_json::json!({})).is_ok()
    }

    pub fn status() -> Result<String, String> {
        let resp = request("status", serde_json::json!({}))?;
        let mode = resp
            .get("mode")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let pid = resp
            .get("kernel")
            .and_then(|k| k.get("pid"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let service = resp.get("service").and_then(|v| v.as_str()).unwrap_or("-");
        Ok(format!(
            "service={SERVICE_LABEL} v={service} mode={mode} kernel_pid={pid}"
        ))
    }

    pub fn state() -> ServiceState {
        let installed = is_installed();
        match request("status", serde_json::json!({})) {
            Ok(resp) => ServiceState {
                installed,
                ready: true,
                mode: resp
                    .get("mode")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string(),
                kernel_pid: resp
                    .get("kernel")
                    .and_then(|k| k.get("pid"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32,
                status: "ok".to_string(),
            },
            Err(err) => ServiceState {
                installed,
                ready: false,
                mode: "unknown".to_string(),
                kernel_pid: 0,
                status: err,
            },
        }
    }

    /// 首次安装（提权一次）：跑随包 `exvlan-service --install <uid>`。
    pub fn install(app: &tauri::AppHandle) -> Result<(), String> {
        // 按平台拼文件名：Windows 是 .exe，写死会让错误信息本身就无法执行。
        let bin = if cfg!(windows) {
            "exvlan-service.exe"
        } else {
            "exvlan-service"
        };
        let src = bundled_service(app)
            .ok_or_else(|| format!("安装包内未找到服务二进制（resources/service/{bin}）"))?;
        let uid = current_uid().to_string();
        // Windows：把**当前用户**的 SID 交给安装器，服务据此把命名管道 DACL
        // 收窄到本人（服务以 LocalSystem 跑，事后反推不出是谁装的）。
        // 取不到就传空串 → 服务退回 `IU`（不会锁门）。
        let sid = exvlan_service_client::current_user_sid().unwrap_or_default();
        let tools = bundled_tools_dir(app)
            .map(|d| d.display().to_string())
            .unwrap_or_default();
        crate::run_elevated(&[
            "--install-exvlan-service",
            &src.display().to_string(),
            &uid,
            &sid,
            &tools,
        ])?;
        // 安装脚本内部已等 socket 就绪，这里再兜一次（避免 launchd 抖动）。
        for _ in 0..25 {
            if is_ready() {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        Err("服务已安装但 IPC 未就绪（见诊断包 service 段）".to_string())
    }

    /// 免密升级服务本体：把新二进制交给运行中的服务，由它自己替换并重启。
    /// 返回 `true` = 确实做了替换。
    pub fn update(app: &tauri::AppHandle) -> Result<bool, String> {
        let want = crate::BUILD_VERSION;
        if installed_version().as_deref() == Some(want) {
            return Ok(false);
        }
        let src = bundled_service(app).ok_or_else(|| "安装包内未找到服务二进制".to_string())?;
        let sig = sig_path(&src);
        if !sig.is_file() {
            return Err(format!("缺少服务签名 {}", sig.display()));
        }
        let staging = std::env::temp_dir().join(format!(
            "era-exvlan-service-{}-{}",
            std::process::id(),
            crate::logger::timestamp().replace(':', "-")
        ));
        std::fs::create_dir_all(&staging).map_err(|e| format!("创建 staging 失败: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o700));
        }
        let staged = staging.join(if cfg!(windows) {
            "exvlan-service.exe"
        } else {
            "exvlan-service"
        });
        std::fs::copy(&src, &staged).map_err(|e| e.to_string())?;
        std::fs::copy(&sig, staging.join("service.sig")).map_err(|e| e.to_string())?;

        let result = request(
            "service.update",
            serde_json::json!({
                "staging": staging.display().to_string(),
                "version": want,
            }),
        );
        let _ = std::fs::remove_dir_all(&staging);
        result?;

        // 服务侧只负责「拉住 updater 并立即返回」，真正的替换是异步的。
        // 这里必须确认版本真的变了，否则会在「装不上 → 下次启动又重装」
        // 之间死循环（pre.14 实测每 3 秒一次 sc stop/start）。
        for _ in 0..30 {
            if installed_version().as_deref() == Some(want) {
                return Ok(true);
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        Err(format!(
            "服务自更新后版本仍为 {:?}（期望 {want}，见 service.log），改走提权安装",
            installed_version()
        ))
    }

    pub fn set_mode(mode: &str) -> Result<(), String> {
        ensure_session()?;
        request("mode.set", serde_json::json!({ "mode": mode })).map(|_| ())
    }

    pub fn ensure_kernel() -> Result<u32, String> {
        ensure_session()?;
        let resp = request("kernel.ensure", serde_json::json!({}))?;
        Ok(resp
            .get("kernel")
            .and_then(|k| k.get("pid"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32)
    }

    pub fn stop_kernel() -> Result<(), String> {
        request("kernel.stop", serde_json::json!({})).map(|_| ())
    }

    /// 原子重启内核（停 → 清残留 → 起），期间服务端会挂起 normal 的停机逻辑。
    pub fn restart_kernel() -> Result<serde_json::Value, String> {
        request("kernel.restart", serde_json::json!({}))
    }

    pub fn kernel_status() -> Result<(u32, bool), String> {
        let resp = request("status", serde_json::json!({}))?;
        let pid = resp
            .get("kernel")
            .and_then(|k| k.get("pid"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        Ok((pid, pid != 0))
    }

    /// 普通模式：建 session → mode=normal → 确保内核在跑。
    pub fn ensure_normal(_app: &tauri::AppHandle) -> Result<String, String> {
        set_mode("normal")?;
        ensure_kernel()?;
        Ok("127.0.0.1:1995".to_string())
    }

    /// 服务模式：建 session → mode=service → 确保内核在跑。
    pub fn ensure_service(_app: &tauri::AppHandle) -> Result<String, String> {
        set_mode("service")?;
        ensure_kernel()?;
        Ok("127.0.0.1:1995".to_string())
    }

    pub fn stop() {
        let mut guard = SESSION.lock().unwrap_or_else(|e| e.into_inner());
        *guard = None; // 断开连接：normal 模式下服务会停内核
    }

    /// 生效的 RPC 白名单（回环 + 各虚拟网段 + 额外追加）。
    pub fn whitelist_info() -> Result<serde_json::Value, String> {
        let resp = request("whitelist.get", serde_json::json!({}))?;
        Ok(resp.get("data").cloned().unwrap_or_default())
    }

    /// 设置额外网段；服务端会做 CIDR 校验并拒绝公网段。
    pub fn set_extra_whitelist(extra: &[String]) -> Result<serde_json::Value, String> {
        let resp = request("whitelist.set", serde_json::json!({ "extra": extra }))?;
        Ok(resp.get("data").cloned().unwrap_or_default())
    }

    /// 生成本机一次性配对码（对端出码；服务托管时由服务调本机 gateway）。
    pub fn pair_start(role: &str, ttl: u64) -> Result<serde_json::Value, String> {
        let resp = request(
            "gateway.pair.start",
            serde_json::json!({ "role": role, "ttl": ttl }),
        )?;
        Ok(resp.get("data").cloned().unwrap_or_default())
    }

    /// 谁占着 RPC 端口（诊断用）。
    pub fn port_owner() -> Result<serde_json::Value, String> {
        let resp = request("port.owner", serde_json::json!({}))?;
        Ok(resp.get("data").cloned().unwrap_or_default())
    }

    /// 结束占用 RPC 端口的**自家内核**（服务端会校验路径，非自家拒绝）。
    pub fn port_cleanup() -> Result<(), String> {
        request("port.cleanup", serde_json::json!({})).map(|_| ())
    }

    /// 免密换内核：把随包内核 + 签名 + 版本放进 staging，再让服务 apply。
    pub fn apply_bundled(app: &tauri::AppHandle) -> Result<(), String> {
        let kernel = crate::kernel::bundled_kernel(app).ok_or("未找到随包内核")?;
        let sig = sig_path(&kernel);
        if !sig.is_file() {
            return Err(format!("缺少内核签名 {}", sig.display()));
        }
        let staging = std::env::temp_dir().join(format!(
            "era-exvlan-apply-{}-{}",
            std::process::id(),
            crate::logger::timestamp().replace(':', "-")
        ));
        std::fs::create_dir_all(&staging).map_err(|e| format!("创建 staging 失败: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o700));
        }
        std::fs::copy(&kernel, staging.join("kernel")).map_err(|e| e.to_string())?;
        std::fs::copy(&sig, staging.join("kernel.sig")).map_err(|e| e.to_string())?;
        std::fs::write(staging.join("version"), crate::BUILD_VERSION).map_err(|e| e.to_string())?;

        let body = serde_json::json!({ "staging": staging.display().to_string() });
        // 服务正在重启（例如刚做完自更新）时会读到空响应；等它起来再试一次。
        let result = match request("apply", body.clone()) {
            Err(err)
                if err.contains("空响应") || err.contains("os error 2") || err.contains("拒绝") =>
            {
                std::thread::sleep(std::time::Duration::from_millis(1500));
                request("apply", body)
            }
            other => other,
        };
        let _ = std::fs::remove_dir_all(&staging);
        result.map(|_| ())
    }
}

// ---------------------------------------------------------------------------
// Linux：没有常驻服务，普通模式交给 supervisor（setcap + PDEATHSIG）
// ---------------------------------------------------------------------------

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod stub {
    use super::*;

    pub fn is_installed() -> bool {
        false
    }
    pub fn installed_version() -> Option<String> {
        None
    }
    pub fn is_ready() -> bool {
        true
    }
    pub fn status() -> Result<String, String> {
        Ok("unsupported (linux: systemd / setcap)".to_string())
    }
    pub fn state() -> ServiceState {
        ServiceState {
            installed: false,
            ready: true,
            mode: "service".to_string(),
            kernel_pid: 0,
            status: "unsupported".to_string(),
        }
    }
    pub fn install(_app: &tauri::AppHandle) -> Result<(), String> {
        Ok(())
    }
    pub fn update(_app: &tauri::AppHandle) -> Result<bool, String> {
        Ok(false)
    }
    pub fn set_mode(_mode: &str) -> Result<(), String> {
        Ok(())
    }
    pub fn ensure_kernel() -> Result<u32, String> {
        Ok(0)
    }
    pub fn stop_kernel() -> Result<(), String> {
        Ok(())
    }
    pub fn restart_kernel() -> Result<serde_json::Value, String> {
        Err("当前平台没有特权服务（core 模式由 systemd/GUI supervisor 管理内核）".to_string())
    }
    pub fn kernel_status() -> Result<(u32, bool), String> {
        Ok((0, false))
    }
    /// 移动端内核在进程内（FFI），没有可拉起的子进程。
    #[cfg(any(target_os = "android", target_os = "ios"))]
    pub fn ensure_normal(_app: &tauri::AppHandle) -> Result<String, String> {
        Ok("ffi".to_string())
    }
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub fn ensure_normal(app: &tauri::AppHandle) -> Result<String, String> {
        crate::supervisor::ensure(app).map_err(|e| format!("{e:#}"))
    }
    pub fn ensure_service(_app: &tauri::AppHandle) -> Result<String, String> {
        Ok("127.0.0.1:1995".to_string())
    }
    #[cfg(any(target_os = "android", target_os = "ios"))]
    pub fn stop() {}
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    pub fn stop() {
        crate::supervisor::stop();
    }
    pub fn apply_bundled(_app: &tauri::AppHandle) -> Result<(), String> {
        Err("Linux 没有特权服务（换内核走提权）".to_string())
    }
    pub fn whitelist_info() -> Result<serde_json::Value, String> {
        Err("当前平台没有特权服务".to_string())
    }
    pub fn set_extra_whitelist(_extra: &[String]) -> Result<serde_json::Value, String> {
        Err("当前平台没有特权服务".to_string())
    }
    /// 裸核平台没有特权服务托管 gateway：配对码用 `exvlan-cli auth pair` 直连 gateway。
    pub fn pair_start(_role: &str, _ttl: u64) -> Result<serde_json::Value, String> {
        Err("当前平台没有特权服务：请用 `exvlan-cli auth pair` 生成配对码".to_string())
    }
    pub fn port_owner() -> Result<serde_json::Value, String> {
        Err("当前平台没有特权服务".to_string())
    }
    pub fn port_cleanup() -> Result<(), String> {
        Err("当前平台没有特权服务".to_string())
    }
}
