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

/// 系统 LaunchDaemons 目录（macOS）。
#[cfg(target_os = "macos")]
pub fn launch_daemons_dir() -> PathBuf {
    PathBuf::from("/Library/LaunchDaemons")
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

/// 旧版本（≤ 7.2）布局：**只用于一次性清理**，不读取、不拷贝、不恢复。
///
/// 7.3 路径 v3 硬切后这些路径不再被任何代码使用；保留这份清单是为了覆盖
/// 「降级回 7.2 再升回来」时旧目录/旧服务被重新创建的情况。7.4 再删除。
pub fn stale_paths() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let mut out = vec![
            PathBuf::from("/Library/Application Support/exvlan"),
            PathBuf::from("/Library/Logs/exvlan"),
            PathBuf::from("/Library/LaunchDaemons/exvlan-studio.plist"),
            PathBuf::from("/Library/LaunchDaemons/exvlan-studio.plist.pre-7.2"),
            PathBuf::from("/var/log/era.exvlan.studio.helper.log"),
        ];
        if let Some(home) = std::env::var_os("HOME") {
            out.push(
                PathBuf::from(home)
                    .join("Library/Application Support/era.exvlan.studio/ui-state.json"),
            );
        }
        out
    }
    #[cfg(target_os = "windows")]
    {
        vec![PathBuf::from(r"C:\ProgramData\exvlan")]
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        vec![
            PathBuf::from("/var/lib/exvlan"),
            PathBuf::from("/opt/exvlan"),
            PathBuf::from("/var/log/exvlan"),
            PathBuf::from("/etc/systemd/system/exvlan-studio.service"),
        ]
    }
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
