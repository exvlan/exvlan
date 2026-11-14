//! 全仓唯一的路径定义（规范见 `docs/paths.md`）。
//!
//! 三端各自遵循自己的系统惯例，但目录名与结构统一：
//!
//! ```text
//! <root>/config/   core/   logs/   diagnostics/   tmp/
//! ```
//!
//! **除本文件外，任何地方不得出现绝对路径字面量**（CI 守卫校验）。

use std::path::PathBuf;

/// 唯一标识：bundle id / applicationId / 服务 label / 目录名 四者一致。
pub const APP_ID: &str = "era.exvlan.studio";

/// 特权服务 label（macOS LaunchDaemon / Windows 服务名）。
/// 记住：它 **不是** `APP_ID`（旧的内核 daemon 服务用的才是 `APP_ID`）。
pub const PRIVILEGED_SERVICE_LABEL: &str = "era.exvlan.studio.service";

/// 系统侧（root 持有）的路径。
pub struct SystemPaths {
    /// 服务配置目录。
    pub config: PathBuf,
    /// 内核二进制与 `version`。
    pub core: PathBuf,
    /// 系统日志目录。
    pub logs: PathBuf,
}

pub fn system() -> SystemPaths {
    #[cfg(target_os = "macos")]
    {
        let data = PathBuf::from("/Library/Application Support").join(APP_ID);
        SystemPaths {
            config: data.join("config"),
            core: data.join("core"),
            logs: PathBuf::from("/Library/Logs").join(APP_ID),
        }
    }
    #[cfg(target_os = "windows")]
    {
        let data = PathBuf::from(r"C:\ProgramData").join(APP_ID);
        SystemPaths {
            config: data.join("config"),
            core: data.join("core"),
            logs: data.join("logs"),
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        SystemPaths {
            config: PathBuf::from("/var/lib").join(APP_ID).join("config"),
            core: PathBuf::from("/opt").join(APP_ID).join("core"),
            logs: PathBuf::from("/var/log").join(APP_ID),
        }
    }
}

/// 特权服务日志（`exvlan-service` 自轮转）。GUI 侧只读，用于诊断包。
pub fn service_log() -> PathBuf {
    system().logs.join("service.log")
}

/// 服务（内核daemon）的 launchd plist（仅 macOS）。
#[cfg(target_os = "macos")]
pub fn service_plist(label: &str) -> Option<PathBuf> {
    Some(PathBuf::from("/Library/LaunchDaemons").join(format!("{label}.plist")))
}
#[cfg(not(target_os = "macos"))]
#[allow(dead_code)]
pub fn service_plist(_label: &str) -> Option<PathBuf> {
    None
}

/// 特权服务（`exvlan-service`）的二进制落点。
///
/// 与 `exvlan/service` 里 `paths()` 的实现**必须一致**：GUI 用这个
/// 路径判断「是否已安装」并读取 `--version`。
pub fn privileged_service_bin() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/Library/PrivilegedHelperTools").join(PRIVILEGED_SERVICE_LABEL)
    }
    #[cfg(target_os = "windows")]
    {
        PathBuf::from(r"C:\ProgramData")
            .join(APP_ID)
            .join("bin")
            .join("exvlan-service.exe")
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        PathBuf::from("/usr/local/bin/exvlan-service")
    }
}

/// 特权服务的 launchd plist（macOS；其它平台返回空路径）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn privileged_service_plist() -> PathBuf {
    service_plist(PRIVILEGED_SERVICE_LABEL).unwrap_or_default()
}

/// 特权服务的 unix socket（macOS）。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn privileged_service_socket() -> PathBuf {
    PathBuf::from("/var/run").join(APP_ID).join("service.sock")
}

/// 系统 LaunchDaemons 目录（macOS）。
#[cfg(target_os = "macos")]
pub fn launch_daemons_dir() -> PathBuf {
    PathBuf::from("/Library/LaunchDaemons")
}

/// 用户侧配置目录（prefs.toml / machine_id / 迁移标记）。
pub fn user_config_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    use tauri::Manager as _;
    app.path()
        .app_config_dir()
        .map_err(|e| format!("无法定位应用配置目录: {e}"))
}

/// 用户侧日志目录。
pub fn user_log_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    use tauri::Manager as _;
    app.path()
        .app_log_dir()
        .map_err(|e| format!("无法定位应用日志目录: {e}"))
}

/// 普通模式（用户态子进程）的配置目录：与系统服务完全隔离。
pub fn normal_config_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(user_config_dir(app)?.join("normal").join("config"))
}

/// 普通模式的日志目录（与系统服务日志分开，诊断时一眼能区分）。
pub fn normal_log_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(user_log_dir(app)?.join("normal"))
}

/// 诊断包落点：唯一目录，不再有 fallback 链。
pub fn diagnostics_dir(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    Ok(user_log_dir(app)?.join("diagnostics"))
}

/// 临时目录：`$TMPDIR/era.exvlan.studio`（macOS 上是 per-user 的 `$TMPDIR`，不是 `/tmp`）。
pub fn tmp_dir() -> PathBuf {
    std::env::temp_dir().join(APP_ID)
}

/// 提权过程日志（放在临时目录下，0600）。
pub fn elevate_log() -> PathBuf {
    tmp_dir().join("elevate.log")
}

/// 收紧文件权限（Unix）：密钥类文件（prefs / networks / 提权日志）用 0600。
pub fn restrict_file(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

/// 设置目录权限（Unix）。`mode` 用八进制字面量，如 `0o700`。
/// Linux：把 root 创建的目录交给桌面用户。
///
/// Linux 没有特权服务进程（内核由 systemd 以 root 运行），但配置文件必须由
/// 用户态的 GUI / CLI 写。目录改成属主所有（权限不变，仍是 0700）：
/// root 内核照样能读，用户终于能写。
#[cfg(all(unix, not(target_os = "macos")))]
pub fn hand_over_to_owner(path: &std::path::Path, uid: u32) {
    use std::os::unix::ffi::OsStrExt as _;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return;
    };
    // gid = 0（保持 root 组），只换属主；失败由调用方记录。
    unsafe {
        libc::chown(c.as_ptr(), uid, 0);
    }
    if let Ok(meta) = std::fs::metadata(path) {
        use std::os::unix::fs::MetadataExt as _;
        if meta.uid() != uid {
            crate::logger::record(
                "paths.owner",
                format!("{} 属主仍是 {}（期望 {uid}）", path.display(), meta.uid()),
            );
        }
    }
}

pub fn ensure_dir_mode(path: &std::path::Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
}
