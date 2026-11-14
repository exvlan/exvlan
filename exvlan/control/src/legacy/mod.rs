//! 旧版本（≤ 7.2）遗留数据检测：**唯一一份**判定来源。
//!
//! 消费方：
//! - GUI 的迁移闸门（`packages/studio/src-tauri/src/migrate/desktop.rs`）——
//!   决定要不要在启动时阻塞并提示跑 `exvlan-migrate`；
//! - `doctor` 的 `migrate.legacy` 检查；
//! - `exvlan-migrate` 自己 —— 真正动手的那一侧。
//!
//! 为什么放在 `exvlan-control` 而不是 `exvlan/migrate`：migrate 只有 `[[bin]]`，
//! GUI 依赖一个 bin crate 不成立；control 是 GUI/CLI/gateway 共用的基础层，
//! 且它已经是「允许出现绝对路径字面量」的白名单文件（`script/ci/check-paths.sh`）。
//!
//! 判定口径：**只有 `exvlan-migrate` 真能处理掉的东西才算遗留**。检测不上报
//! 「迁移工具无从下手」的东西（例如彻底坏掉的 `[app].mode`），否则闸门会永远
//! pending。

pub mod app_mode;
pub mod service_toml;

use std::path::{Path, PathBuf};

use crate::paths::{Paths, UserPaths};

/// 遗留项种类（报告 / 闸门展示用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    NetworksToml,
    ServiceJson,
    ConfigDir,
    UiState,
    KernelPrev,
    PrefsAppMode,
    ServiceToml,
    MachineId,
    StalePath,
    LegacyService,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::NetworksToml => "networks_toml",
            Kind::ServiceJson => "service_json",
            Kind::ConfigDir => "config_dir",
            Kind::UiState => "ui_state",
            Kind::KernelPrev => "kernel_prev",
            Kind::PrefsAppMode => "prefs_app_mode",
            Kind::ServiceToml => "service_toml",
            Kind::MachineId => "machine_id",
            Kind::StalePath => "stale_path",
            Kind::LegacyService => "legacy_service",
        }
    }
}

/// 一条遗留项：给人看的路径 + 为什么它算遗留。
#[derive(Debug, Clone)]
pub struct Item {
    pub kind: Kind,
    pub target: String,
    pub hint: String,
}

impl Item {
    /// 闸门列表里展示的一行。
    pub fn label(&self) -> String {
        if self.hint.is_empty() {
            self.target.clone()
        } else {
            format!("{} — {}", self.target, self.hint)
        }
    }
}

/// `[app].mode` 里的旧 JSON 与 `[misc]` 里的陈旧键。
#[derive(Debug, Clone, Default)]
pub struct PrefsLegacy {
    /// `prefs.toml` 路径（不一定存在）。
    pub path: PathBuf,
    /// `[app].mode` 原值（`None` = 没有这个键）。
    pub app_mode: Option<String>,
    /// `[misc]` 里前端写穿留下的陈旧键。
    pub stale_misc_keys: Vec<String>,
    /// `[misc].remotes` 是否已有值（决定远端列表是否还需要合成）。
    pub has_remotes: bool,
    /// `[misc].runtime` 是否已有值。
    pub has_runtime: bool,
}

impl PrefsLegacy {
    /// `[app].mode` 是否还需要拆分。
    ///
    /// 不能只看「有没有 `mode` 键」：7.3 的前端仍会把完整 `app_mode` 写进
    /// `[app].mode`，那样会永远 pending。判据是「拆分的目标键还没落地」：
    /// - 没有 `runtime` 且旧 JSON 里还有被消费的键；或
    /// - 旧 JSON 是 `remote` 且合成的 `remotes` 还没写进 `[misc]`（否则丢远端列表）。
    pub fn app_mode_needs_split(&self) -> bool {
        let Some(raw) = self.app_mode.as_deref() else {
            return false;
        };
        if !app_mode::has_consumed_keys(raw) {
            return false;
        }
        if !self.has_runtime {
            return true;
        }
        match app_mode::split(raw) {
            app_mode::AppModeOutcome::Splittable(split) => {
                !split.remotes.is_empty() && !self.has_remotes
            }
            app_mode::AppModeOutcome::Invalid(_) => false,
        }
    }
}

/// `machine_id` 归位计划：新路径不存在、旧路径存在时才产生。
#[derive(Debug, Clone)]
pub struct MachineIdPlan {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// 旧服务定义（launchd label / SCM 服务名 / 计划任务）。
#[derive(Debug, Clone)]
pub struct LegacyService {
    pub name: String,
    /// `launchd` / `scm` / `task`
    pub kind: &'static str,
    /// 探测到的落点（plist / 任务 XML），SCM 服务没有文件时为 `None`。
    pub path: Option<PathBuf>,
}

/// 一次完整检测的结果。
#[derive(Debug, Clone, Default)]
pub struct Legacy {
    pub networks_toml: Option<PathBuf>,
    pub service_json: Option<PathBuf>,
    pub config_d: Option<PathBuf>,
    pub ui_state: Vec<PathBuf>,
    pub prev_kernels: Vec<PathBuf>,
    pub prefs: PrefsLegacy,
    /// `service.toml` 里还有旧字段名 / 旧词汇（需要 S8 归一）。
    pub service_toml: Option<PathBuf>,
    pub machine_id: Option<MachineIdPlan>,
    pub stale_paths: Vec<PathBuf>,
    pub services: Vec<LegacyService>,
    pub items: Vec<Item>,
}

impl Legacy {
    /// 是否需要迁移（闸门用）：`items` 为空即无遗留。
    pub fn pending(&self) -> bool {
        !self.items.is_empty()
    }

    /// 闸门列表（路径 + 说明），一条一行。
    pub fn item_labels(&self) -> Vec<String> {
        self.items.iter().map(Item::label).collect()
    }
}

/// 选出旧 `service.json`：用户目录优先，其次**系统目录**。
///
/// 系统目录这一处（macOS/Windows 的服务配置，root 持有）在 pre.17 首个版本被漏掉，
/// 结果是升级后 `owner_uid` / `policy` / `rpc_extra_whitelist` 被重置成默认值。
pub fn pick_service_json(user_data: &Path, service_config: &Path) -> Option<PathBuf> {
    [
        user_data.join("service.json"),
        service_config.with_file_name("service.json"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

/// 全量检测（只读，不产生副作用）。
pub fn detect(user: &UserPaths, p: &Paths) -> Legacy {
    let mut legacy = Legacy {
        prefs: read_prefs_legacy(&user.data),
        ..Legacy::default()
    };

    let networks = user.data.join("networks.toml");
    if networks.is_file() {
        legacy.networks_toml = Some(networks);
    }
    legacy.service_json = pick_service_json(&user.data, &p.service_config);
    let config_d = user.data.join("config.d");
    if config_d.is_dir() {
        legacy.config_d = Some(config_d);
    }
    for name in ["ui-state.json", "ui-state.json.bak"] {
        let path = user.data.join(name);
        if path.is_file() {
            legacy.ui_state.push(path);
        }
    }
    for name in ["exvlan.prev", "exvlan.exe.prev"] {
        let path = p.core.join(name);
        if path.is_file() {
            legacy.prev_kernels.push(path);
        }
    }
    legacy.service_toml = service_toml_needs_normalization(&p.service_config);
    legacy.machine_id = machine_id_plan(&p.core);
    legacy.stale_paths = crate::paths::stale_paths()
        .into_iter()
        .filter(|path| path.exists())
        .collect();
    legacy.services = detect_legacy_services();

    legacy.items = build_items(&legacy);
    legacy
}

fn build_items(legacy: &Legacy) -> Vec<Item> {
    fn push(items: &mut Vec<Item>, kind: Kind, target: String, hint: &str) {
        items.push(Item {
            kind,
            target,
            hint: hint.to_string(),
        });
    }

    let mut items = Vec::new();
    if let Some(path) = &legacy.networks_toml {
        push(
            &mut items,
            Kind::NetworksToml,
            path.display().to_string(),
            "旧网络列表，转换为内核 config/*.toml",
        );
    }
    if let Some(path) = &legacy.service_json {
        push(
            &mut items,
            Kind::ServiceJson,
            path.display().to_string(),
            "旧服务配置，转换为 service.toml",
        );
    }
    if let Some(path) = &legacy.config_d {
        push(
            &mut items,
            Kind::ConfigDir,
            path.display().to_string(),
            "旧配置目录，合并进内核 config/",
        );
    }
    for path in &legacy.ui_state {
        let hint = if path.extension().is_some_and(|ext| ext == "bak") {
            "7.1 损坏快照，只记录不恢复"
        } else {
            "旧界面状态，导入 prefs.toml"
        };
        push(&mut items, Kind::UiState, path.display().to_string(), hint);
    }
    for path in &legacy.prev_kernels {
        push(
            &mut items,
            Kind::KernelPrev,
            path.display().to_string(),
            "旧内核备份，删除",
        );
    }
    if legacy.prefs.app_mode_needs_split() {
        push(
            &mut items,
            Kind::PrefsAppMode,
            format!("{} [app].mode", legacy.prefs.path.display()),
            "旧 app_mode，拆成 runtime / target / remotes",
        );
    }
    if let Some(path) = &legacy.service_toml {
        push(
            &mut items,
            Kind::ServiceToml,
            path.display().to_string(),
            "旧字段名/旧词汇（mode → policy），归一后再删运行时别名",
        );
    }
    if let Some(plan) = &legacy.machine_id {
        push(
            &mut items,
            Kind::MachineId,
            plan.from.display().to_string(),
            &format!("旧 machine_id，复制到 {}", plan.to.display()),
        );
    }
    for path in &legacy.stale_paths {
        push(
            &mut items,
            Kind::StalePath,
            path.display().to_string(),
            "7.2 及更早的路径，删除",
        );
    }
    for service in &legacy.services {
        let hint = format!(
            "旧{}定义，停用并删除",
            match service.kind {
                "launchd" => " launchd ",
                "scm" => " Windows 服务",
                "task" => "计划任务",
                _ => "",
            }
        );
        push(
            &mut items,
            Kind::LegacyService,
            service
                .path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| service.name.clone()),
            &hint,
        );
    }
    items
}

/// `prefs.toml` 里与迁移相关的部分（读不到/坏文件时返回默认值，交由 migrate 报错）。
pub fn read_prefs_legacy(data: &Path) -> PrefsLegacy {
    let path = data.join("prefs.toml");
    let mut out = PrefsLegacy {
        path: path.clone(),
        ..PrefsLegacy::default()
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return out;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&text) else {
        return out;
    };
    if let Some(raw) = value
        .get("app")
        .and_then(|app| app.get("mode"))
        .and_then(|mode| mode.as_str())
    {
        out.app_mode = Some(raw.to_string());
    }
    if let Some(misc) = value.get("misc").and_then(|m| m.as_table()) {
        out.has_remotes = misc.contains_key("remotes");
        out.has_runtime = misc.contains_key("runtime");
        for key in STALE_MISC_KEYS {
            if misc.contains_key(*key) {
                out.stale_misc_keys.push((*key).to_string());
            }
        }
    }
    out
}

/// 前端写穿留下的陈旧 `[misc]` 键（值为界面缓存，不是设置）。
pub const STALE_MISC_KEYS: &[&str] = &["networkList", "upgrade_ignored_version"];

/// `service.toml` 是否需要 S8 归一。
///
/// 读不到（普通用户读不了 root 0600 的文件）时返回 `None`：**不猜**。提权后的
/// `exvlan-migrate` 自己会再读一次并归一，闸门只负责「看得见的时候提醒」。
fn service_toml_needs_normalization(path: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(path).ok()?;
    service_toml::needs_normalization(&text).then(|| path.to_path_buf())
}

fn machine_id_plan(core: &Path) -> Option<MachineIdPlan> {
    let to = crate::paths::machine_id_state_dir()?.join("machine_id");
    if to.exists() {
        return None;
    }
    let from = crate::paths::legacy_machine_id_sources(core)
        .into_iter()
        .find(|path| path.is_file())?;
    if from == to {
        return None;
    }
    Some(MachineIdPlan { from, to })
}

/// 旧服务定义探测（macOS launchd / Windows SCM + 计划任务）。
///
/// 只做「看得见」的探测：
/// - macOS：plist 文件；旧 helper 二进制 / socket；
/// - Windows：`sc query`（注册表里的服务）+ 计划任务 XML 文件。
pub fn detect_legacy_services() -> Vec<LegacyService> {
    #[cfg(target_os = "macos")]
    {
        let mut out = Vec::new();
        for label in crate::paths::LEGACY_LAUNCHD_LABELS {
            for dir in crate::paths::launchd_dirs() {
                let plist = dir.join(format!("{label}.plist"));
                if plist.is_file() {
                    out.push(LegacyService {
                        name: (*label).to_string(),
                        kind: "launchd",
                        path: Some(plist),
                    });
                }
            }
        }
        for bin in crate::paths::legacy_helper_bins() {
            if bin.is_file() {
                out.push(LegacyService {
                    name: bin
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    kind: "launchd",
                    path: Some(bin),
                });
            }
        }
        let socket = crate::paths::legacy_helper_socket();
        if socket.exists() {
            out.push(LegacyService {
                name: "helper.sock".into(),
                kind: "launchd",
                path: Some(socket),
            });
        }
        out
    }
    #[cfg(target_os = "windows")]
    {
        let mut out = Vec::new();
        for name in crate::paths::LEGACY_SCM_SERVICES {
            if scm_service_exists(name) {
                out.push(LegacyService {
                    name: (*name).to_string(),
                    kind: "scm",
                    path: None,
                });
            }
        }
        let dir = crate::paths::scheduled_tasks_dir();
        for name in crate::paths::LEGACY_SCHEDULED_TASKS {
            let xml = dir.join(name);
            if xml.is_file() {
                out.push(LegacyService {
                    name: (*name).to_string(),
                    kind: "task",
                    path: Some(xml),
                });
            }
        }
        out
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // Linux 的旧单元文件在 `stale_paths()` 里，没有额外的服务定义要清。
        Vec::new()
    }
    #[cfg(not(any(unix, target_os = "windows")))]
    {
        Vec::new()
    }
}

/// `sc query <name>` 能打开即视为存在（不存在时 SC 返回 1060 / 非 0 退出码）。
#[cfg(target_os = "windows")]
pub fn scm_service_exists(name: &str) -> bool {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    use std::os::windows::process::CommandExt as _;
    std::process::Command::new("sc")
        .args(["query", name])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "exvlan-legacy-test-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn app_mode_needs_split_until_runtime_lands() {
        let mut prefs = PrefsLegacy {
            app_mode: Some(r#"{"mode":"service","config_dir":"/old"}"#.into()),
            ..PrefsLegacy::default()
        };
        assert!(prefs.app_mode_needs_split());
        prefs.has_runtime = true;
        // service 模式拆完只剩 runtime，不再 pending（前端以后重写完整 app_mode 也不会再报）
        assert!(!prefs.app_mode_needs_split());
    }

    #[test]
    fn remote_mode_needs_split_until_remotes_land() {
        let raw = format!(
            r#"{{"mode":"remote","remote_rpc_address":"10.0.0.9:{}"}}"#,
            crate::DEFAULT_RPC_PORT
        );
        let mut prefs = PrefsLegacy {
            app_mode: Some(raw),
            has_runtime: true,
            has_remotes: false,
            ..PrefsLegacy::default()
        };
        // runtime 有了但 remotes 没有 → 仍要拆（否则丢远端列表）
        assert!(prefs.app_mode_needs_split());
        prefs.has_remotes = true;
        assert!(!prefs.app_mode_needs_split());
    }

    #[test]
    fn slimmed_app_mode_is_not_legacy() {
        let prefs = PrefsLegacy {
            app_mode: Some(r#"{"config_server_url":"https://cfg.example"}"#.into()),
            ..PrefsLegacy::default()
        };
        assert!(!prefs.app_mode_needs_split());
    }

    #[test]
    fn reads_prefs_app_mode_and_stale_misc_keys() {
        let data = data_dir("prefs");
        std::fs::write(
            data.join("prefs.toml"),
            r#"[app]
mode = '{"mode":"normal"}'

[misc]
networkList = "[]"
upgrade_ignored_version = "26.1"
lang = "cn"
"#,
        )
        .unwrap();
        let prefs = read_prefs_legacy(&data);
        assert_eq!(prefs.app_mode.as_deref(), Some(r#"{"mode":"normal"}"#));
        assert_eq!(
            prefs.stale_misc_keys,
            vec![
                "networkList".to_string(),
                "upgrade_ignored_version".to_string()
            ]
        );
        assert!(!prefs.has_runtime);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn detect_reports_service_json_in_system_dir() {
        // 闸门漏掉这一处的后果：只有系统目录脏的机器 pending=false → 永不提示。
        let data = data_dir("sys-json-data");
        let config_dir = data_dir("sys-json-config");
        std::fs::write(config_dir.join("service.json"), "{}").unwrap();
        let user = UserPaths {
            data: data.clone(),
            logs: data.clone(),
        };
        let mut p = crate::paths::paths();
        p.service_config = config_dir.join("service.toml");
        p.core = config_dir.join("core");
        let legacy = detect(&user, &p);
        assert!(
            legacy
                .items
                .iter()
                .any(|item| item.kind == Kind::ServiceJson),
            "系统目录 service.json 必须被检测到: {:?}",
            legacy.item_labels()
        );
        let _ = std::fs::remove_dir_all(&data);
        let _ = std::fs::remove_dir_all(&config_dir);
    }
}
