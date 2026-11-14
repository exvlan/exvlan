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
    /// 内核版本文件。
    ///
    /// `exvlan-service` 落盘时就写在 `<core dir>/version`（与 GUI 的
    /// `kernel::installed_version()` 同一约定）；这里以前拼的是
    /// `<kernel>.version`，两边不一致 → `doctor` 永远显示 `kernel=-`。
    pub fn kernel_version_file(&self) -> PathBuf {
        self.kernel_bin
            .parent()
            .map(|dir| dir.join("version"))
            .unwrap_or_else(|| with_suffix(&self.kernel_bin, ".version"))
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

/// `$HOME`（macOS/Linux 的用户目录；Windows 用 `%APPDATA%`，不需要它）。
#[cfg(not(target_os = "windows"))]
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// 旧版本（≤ 7.2）布局：**只用于一次性清理**，不读取、不拷贝、不恢复。
///
/// 7.3 路径 v3 硬切后这些路径不再被任何代码使用；保留这份清单是为了覆盖
/// 「降级回 7.2 再升回来」时旧目录/旧服务被重新创建的情况。7.4 再删除。
///
/// 这里是**唯一**一份清单（GUI 的 `cleanup_stale_paths`、`exvlan-migrate`、
/// `doctor` 都引用它）；用户侧的文件（`ui-state.json` / `networks.toml`）不在此列 ——
/// 它们由迁移步骤按「先写出目标、再删源」的规则单独处理，不能在这被无条件删掉。
pub fn stale_paths() -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        vec![
            PathBuf::from("/Library/Application Support/exvlan"),
            PathBuf::from("/Library/Logs/exvlan"),
            PathBuf::from("/Library/LaunchDaemons/exvlan-studio.plist"),
            PathBuf::from("/Library/LaunchDaemons/exvlan-studio.plist.pre-7.2"),
            PathBuf::from("/var/log/era.exvlan.studio.helper.log"),
        ]
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

/// 内核 `machine_id` 的落点目录（与内核 `machine_id.rs` 的
/// `default_machine_id_state_dir()` **必须一致**；不一致会让老用户变成「新设备」：
/// peer_id 变化 → 设备登记 / 已配对 token 全部对不上）。
///
/// 三端与 `user_paths().data` 的关系并不相同，别想当然：
/// - macOS：`~/Library/Application Support/<APP_ID>`（与 data 同目录）
/// - Windows：`%LOCALAPPDATA%\<APP_ID>`（data 在 `%APPDATA%`）
/// - Linux：`$XDG_DATA_HOME/<APP_ID>`（data 在 `$XDG_STATE_HOME`）
pub fn machine_id_state_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        home_dir().map(|home| {
            home.join("Library")
                .join("Application Support")
                .join(APP_ID)
        })
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("LOCALAPPDATA")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .map(|dir| dir.join(APP_ID))
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        data_root().map(|dir| dir.join(APP_ID))
    }
}

/// `$XDG_DATA_HOME`（缺省 `~/.local/share`）：Linux 用户数据根。
#[cfg(all(unix, not(target_os = "macos")))]
fn data_root() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    home_dir().map(|home| home.join(".local").join("share"))
}

/// 7.1 及更早的 `machine_id` 落点（目录名从 `exvlan` 改成 `<APP_ID>` 之前）。
///
/// 顺序即优先级：先 `exvlan/`，再内核二进制旁的 `et_machine_id`。
/// 只用于「新路径不存在时 COPY 过去」，**不删除**源文件。
pub fn legacy_machine_id_sources(core: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    #[cfg(target_os = "macos")]
    {
        if let Some(home) = home_dir() {
            out.push(
                home.join("Library")
                    .join("Application Support")
                    .join("exvlan")
                    .join("machine_id"),
            );
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(dir) = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty()) {
            out.push(PathBuf::from(dir).join("exvlan").join("machine_id"));
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(dir) = data_root() {
            out.push(dir.join("exvlan").join("machine_id"));
        }
        // 以 root 且没有 `HOME` 跑过的内核把状态目录落到 `/var/lib/<id>` /
        // `/var/lib/exvlan`（`default_linux_machine_id_state_dir` 的兜底分支）。
        // 这两处都在 `stale_paths()` 的删除清单里，必须**先**把身份取出来。
        out.push(PathBuf::from("/var/lib").join("exvlan").join("machine_id"));
        out.push(PathBuf::from("/var/lib").join(APP_ID).join("machine_id"));
    }
    // 内核 `settings` 老版本把 machine_id 写在可执行文件旁（`et_machine_id`）。
    out.push(core.join("et_machine_id"));
    out
}

/// 旧 macOS 守护进程 label（当前 label 是 [`PRIVILEGED_SERVICE_LABEL`]，不在列表里）。
///
/// 与 `exvlan/service/src/main.rs::cleanup_legacy_macos()` 同一份清单。
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const LEGACY_LAUNCHD_LABELS: &[&str] = &[
    "era.exvlan.studio",
    "era.exvlan.studio.helper",
    "com.exvlan.studio",
    "com.exvlan.studio.helper",
    "exvlan-studio",
];

/// 旧 Windows SCM 服务名（与 `cleanup_legacy_windows()` 同一份）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub const LEGACY_SCM_SERVICES: &[&str] = &["era.exvlan.studio", "exvlan-studio"];

/// 旧 Windows 计划任务（与 `cleanup_legacy_windows()` 同一份）。
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub const LEGACY_SCHEDULED_TASKS: &[&str] = &["era.exvlan.studio", "ExvlanKernelSupervisor"];

/// macOS LaunchDaemons / LaunchAgents 目录（旧 plist 落点）。
#[cfg(target_os = "macos")]
pub fn launchd_dirs() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/Library/LaunchDaemons"),
        PathBuf::from("/Library/LaunchAgents"),
    ]
}

/// macOS 旧 helper 二进制（pre.12 遗留）。
#[cfg(target_os = "macos")]
pub fn legacy_helper_bins() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/Library/PrivilegedHelperTools/era.exvlan.studio.helper"),
        PathBuf::from("/Library/PrivilegedHelperTools/com.exvlan.studio.helper"),
    ]
}

/// macOS 旧 helper 的运行时 socket（权限曾为 0666）。
#[cfg(target_os = "macos")]
pub fn legacy_helper_socket() -> PathBuf {
    PathBuf::from("/var/run").join(APP_ID).join("helper.sock")
}

/// Windows 计划任务的 XML 落点（探测用；注册表里的服务项探测走 `sc query`）。
#[cfg(target_os = "windows")]
pub fn scheduled_tasks_dir() -> PathBuf {
    PathBuf::from(r"C:\Windows\System32\Tasks")
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
