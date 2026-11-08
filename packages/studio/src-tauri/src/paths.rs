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

/// 系统侧（root 持有）的路径。
pub struct SystemPaths {
    /// 服务配置目录。
    pub config: PathBuf,
    /// 内核二进制与 `version`。
    pub core: PathBuf,
    /// 系统日志目录。
    pub logs: PathBuf,
    /// 运行时状态目录（socket）；非 Unix 平台为 `None`。
    pub run: Option<PathBuf>,
}

pub fn system() -> SystemPaths {
    #[cfg(target_os = "macos")]
    {
        let data = PathBuf::from("/Library/Application Support").join(APP_ID);
        SystemPaths {
            config: data.join("config"),
            core: data.join("core"),
            logs: PathBuf::from("/Library/Logs").join(APP_ID),
            run: Some(PathBuf::from("/var/run").join(APP_ID)),
        }
    }
    #[cfg(target_os = "windows")]
    {
        let data = PathBuf::from(r"C:\ProgramData").join(APP_ID);
        SystemPaths {
            config: data.join("config"),
            core: data.join("core"),
            logs: data.join("logs"),
            run: None,
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        SystemPaths {
            config: PathBuf::from("/var/lib").join(APP_ID).join("config"),
            core: PathBuf::from("/opt").join(APP_ID).join("core"),
            logs: PathBuf::from("/var/log").join(APP_ID),
            run: Some(PathBuf::from("/var/run").join(APP_ID)),
        }
    }
}

/// 服务（内核）日志文件。
pub fn kernel_log() -> PathBuf {
    system().logs.join("exvlan.log")
}

/// helper 日志文件（系统日志根下，与内核日志同级）。
pub fn helper_log() -> PathBuf {
    system().logs.join("helper.log")
}

/// helper 的 UNIX socket（仅 macOS 有 helper）。
pub fn helper_socket() -> Option<PathBuf> {
    system().run.map(|dir| dir.join("helper.sock"))
}

#[cfg(target_os = "macos")]
pub fn helper_bin() -> Option<PathBuf> {
    Some(PathBuf::from("/Library/PrivilegedHelperTools").join(format!("{APP_ID}.helper")))
}
#[cfg(not(target_os = "macos"))]
pub fn helper_bin() -> Option<PathBuf> {
    None
}

#[cfg(target_os = "macos")]
pub fn helper_plist() -> Option<PathBuf> {
    Some(PathBuf::from("/Library/LaunchDaemons").join(format!("{APP_ID}.helper.plist")))
}
#[cfg(not(target_os = "macos"))]
pub fn helper_plist() -> Option<PathBuf> {
    None
}

#[cfg(target_os = "macos")]
pub fn helper_version_file() -> Option<PathBuf> {
    Some(PathBuf::from("/Library/PrivilegedHelperTools").join(format!("{APP_ID}.helper.version")))
}
#[cfg(not(target_os = "macos"))]
pub fn helper_version_file() -> Option<PathBuf> {
    None
}

/// 用户侧配置目录（prefs.toml / networks.toml / machine_id）。
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
