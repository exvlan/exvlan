//! 三平台统一的路径定义。
//!
//! **这里是唯一允许出现绝对路径字面量的地方之一**（另一个是 GUI 的 `paths.rs`，
//! 两者必须保持一致，CI 的 `check-paths.sh` 会校验）。

use std::path::PathBuf;

/// bundle id / 服务 label 前缀 / 目录名，四者一致。
pub const APP_ID: &str = "era.exvlan.studio";
/// 特权服务 label（macOS LaunchDaemon / Windows 服务名）。
pub const PRIVILEGED_SERVICE_LABEL: &str = "era.exvlan.studio.service";

/// 系统侧（root 持有）目录 + 二进制位置。
#[derive(Debug, Clone)]
pub struct Paths {
    /// 内核的 `--config-dir`（网络实例配置）。
    pub config: PathBuf,
    /// 内核二进制与 `version` 文件。
    pub core: PathBuf,
    /// 系统日志（内核 / 服务）。
    pub logs: PathBuf,
    /// 特权服务二进制。
    pub service_bin: PathBuf,
    /// 服务配置文件（pre.17 起为 `service.toml`）。
    pub service_config: PathBuf,
    /// gateway 二进制。
    pub gateway_bin: PathBuf,
    /// CLI 二进制（随包安装，便于 agent 直接调用）。
    pub cli_bin: PathBuf,
    /// 内核二进制。
    pub kernel_bin: PathBuf,
}

impl Paths {
    pub fn kernel_version_file(&self) -> PathBuf {
        with_suffix(&self.kernel_bin, ".version")
    }
    pub fn service_version_file(&self) -> PathBuf {
        with_suffix(&self.service_bin, ".version")
    }
}

fn with_suffix(base: &std::path::Path, suffix: &str) -> PathBuf {
    let mut raw = base.as_os_str().to_os_string();
    raw.push(suffix);
    PathBuf::from(raw)
}

/// 系统路径（与 `exvlan/service` 里的 `paths()` 保持一致）。
pub fn paths() -> Paths {
    #[cfg(target_os = "macos")]
    {
        let data = PathBuf::from("/Library/Application Support").join(APP_ID);
        let core = data.join("core");
        Paths {
            config: data.join("config"),
            kernel_bin: core.join("exvlan"),
            gateway_bin: core.join("exvlan-gateway"),
            cli_bin: core.join("exvlan-cli"),
            core,
            logs: PathBuf::from("/Library/Logs").join(APP_ID),
            service_bin: PathBuf::from("/Library/PrivilegedHelperTools")
                .join(PRIVILEGED_SERVICE_LABEL),
            service_config: data.join("service.toml"),
        }
    }
    #[cfg(target_os = "windows")]
    {
        let data = PathBuf::from(r"C:\ProgramData").join(APP_ID);
        let core = data.join("core");
        Paths {
            config: data.join("config"),
            kernel_bin: core.join("exvlan.exe"),
            gateway_bin: core.join("exvlan-gateway.exe"),
            cli_bin: core.join("exvlan-cli.exe"),
            core,
            logs: data.join("logs"),
            service_bin: data.join("bin").join("exvlan-service.exe"),
            service_config: data.join("service.toml"),
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let data = PathBuf::from("/var/lib").join(APP_ID);
        let core = PathBuf::from("/opt").join(APP_ID).join("core");
        Paths {
            config: data.join("config"),
            kernel_bin: core.join("exvlan"),
            gateway_bin: core.join("exvlan-gateway"),
            cli_bin: core.join("exvlan-cli"),
            core,
            logs: PathBuf::from("/var/log").join(APP_ID),
            service_bin: PathBuf::from("/usr/local/bin/exvlan-service"),
            service_config: data.join("service.toml"),
        }
    }
}

/// 用户侧路径（与应用数据同源，GUI/CLI 都以当前用户身份访问）。
#[derive(Debug, Clone)]
pub struct UserPaths {
    /// 用户数据目录（`prefs.toml` / `migrate.done` / gateway token 等）。
    pub data: PathBuf,
    /// 用户日志目录。
    pub logs: PathBuf,
}

/// 用户目录（三平台一致命名）。
pub fn user_paths() -> UserPaths {
    #[cfg(target_os = "macos")]
    {
        let home = home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
        UserPaths {
            data: home
                .join("Library")
                .join("Application Support")
                .join(APP_ID),
            logs: home.join("Library").join("Logs").join(APP_ID),
        }
    }
    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Users\Public"));
        let local = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| appdata.clone());
        UserPaths {
            data: appdata.join(APP_ID),
            logs: local.join(APP_ID).join("logs"),
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let state = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|h| h.join(".local").join("state")))
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        let data = state.join(APP_ID);
        UserPaths {
            data: data.clone(),
            logs: data,
        }
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// 服务 IPC 的 socket / 命名管道地址。
pub fn service_ipc_addr() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        PathBuf::from(r"\\.\pipe\era.exvlan.studio.service")
    }
    #[cfg(not(target_os = "windows"))]
    {
        PathBuf::from("/var/run").join(APP_ID).join("service.sock")
    }
}

/// 特权服务的 launchd plist（仅 macOS）。
pub fn service_plist() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/Library/LaunchDaemons").join(format!("{PRIVILEGED_SERVICE_LABEL}.plist"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        PathBuf::new()
    }
}
