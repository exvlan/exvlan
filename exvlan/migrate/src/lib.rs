//! 一次性迁移的**核心**（lib 部分）：`plan()` 只读算出计划，`Plan::apply()` 执行。
//!
//! 为什么拆 lib + bin（`docs/decisions-7.3.md` §14.15）：`exvlan-migrate` 是桌面
//! 二进制，跑不进 Android；移动端首启也要清掉旧 `config/networks.toml`。迁移逻辑
//! 放这里，桌面 CLI（`main.rs`）与移动端 FFI（`exvlan/contrib/ffi`）共用同一份。
//!
//! 分工：
//! - **lib（本文件）**：检测 → 备份清单 → 转换 → `service.toml` 归一 → prefs →
//!   `machine_id` → 旧路径清理 → 删源 → 报告；每一步都带 `status` 与 `reason`；
//! - **bin（`main.rs`）**：参数解析、日志、备份目录布局、提权属主（`chown`）、
//!   桌面旧服务定义清理（`launchctl`/`sc`/`schtasks`）、`--json`/退出码、`--rollback`；
//! - **移动端（[`mobile`]）**：只做 `networks.toml` 转换，语义同上（失败保留源）。
//!
//! 契约见 `docs/migrate-7.3.md`：默认 dry-run、失败保留现场、`--rollback` 可还原。

pub mod cleanup;
pub mod convert;
pub mod machine_id;
pub mod mobile;
pub mod prefs;
pub mod report;
pub mod sanitize;
pub mod service_toml;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use serde_json::{Value, json};

pub use exvlan_control::legacy::{Legacy, LegacyService, MIGRATE_DONE_MARKER};
pub use exvlan_control::paths::{Paths, UserPaths};
pub use report::{Status, Step};

/// `owner_sid` 存量清洗的标记（与 [`MIGRATE_DONE_MARKER`] 同目录 = 用户数据目录）。
///
/// 名字里的 `v2` 是**历史事实**（pre.17 第一个迁移版就用它），不要跟着
/// [`MIGRATE_DONE_MARKER`] 一起改：改了会让已经清洗过的机器再清洗一次
/// （虽然幂等，但没有意义，且会重写 `migrate.done.v2` 的时间戳）。
pub const SANITIZE_MARKER: &str = "migrate.done.v2";

/// 展示版本：优先用 CI 注入的构建串（`<标准版本>[-pre.N][+sha]`），
/// 本地直接 cargo build 时回落到 crate 版本。
pub const TOOL_VERSION: &str = match option_env!("EXVLAN_BUILD_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// 日志级别（CLI 映射到 `migrate.log` 的 `INFO/WARN/ERROR`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

/// 平台差异（桌面 CLI / 移动端 / 测试）。
///
/// 只有这四件事和平台有关；其余全部在 lib 里。
pub trait Host {
    /// 备份点：**必须在任何原地改写之前**调用一次。返回备份目录
    /// （`None` = 当前宿主不备份，例如移动端；dry-run 由调用方直接跳过）。
    fn backup(&mut self, plan: &Plan) -> anyhow::Result<Option<PathBuf>>;

    /// 旧服务定义清理（桌面特权操作：`launchctl` / `sc` / `schtasks`）。
    ///
    /// 返回 `None` 表示宿主没有这一步（移动端 / 测试），lib 会写一条 `skipped`。
    fn legacy_services(&mut self, _services: &[LegacyService], _dry_run: bool) -> Option<Step> {
        None
    }

    /// 进度 / 告警（CLI 写到 `migrate.log` + stderr）。
    fn log(&mut self, _level: Level, _msg: &str) {}

    /// 把新产生的文件属主交给桌面用户（提权运行时才需要）。默认什么都不做。
    fn adopt(&mut self, _path: &Path) {}
}

/// 不做事也不记日志的宿主（lib 测试 / 移动端 / dry-run 走查）。
pub struct NoHost;

impl Host for NoHost {
    fn backup(&mut self, _plan: &Plan) -> anyhow::Result<Option<PathBuf>> {
        Ok(None)
    }
}

/// 一次迁移的计划（**只读**；`plan()` 的结果可以直接 dry-run 走查）。
#[derive(Debug)]
pub struct Plan {
    pub user: UserPaths,
    pub paths: Paths,
    pub legacy: Legacy,
    pub prefs: prefs::PrefsPlan,
    /// `machine_id` 归位计划（`None` = 不需要 / 不可用，原因在 [`Plan::machine_id_step`]）。
    pub machine_id: Option<machine_id::MachineIdPlan>,
    /// `machine_id` 没产生计划时的说明步骤。
    pub machine_id_step: Option<Step>,
}

/// 一次迁移的结论（不打印、不退出进程 —— 那些是 bin 的事）。
#[derive(Debug)]
pub struct Report {
    /// `--json` 打到 stdout 的那份（顶层键与 pre.17 首个版本一致）。
    pub json: Value,
    pub nothing_to_do: bool,
    /// 备份目录（dry-run / 宿主不备份时为 `None`）。
    pub backup: Option<PathBuf>,
    /// 有失败项（`report["ok"] == false`）：marker 里要记下来便于人工复查。
    pub incomplete: Vec<String>,
    pub converted_networks: usize,
    pub removed: Vec<String>,
}

/// 生成计划（桌面：`machine_id` 落点取 [`exvlan_control::paths::machine_id_state_dir`]）。
pub fn plan(user: &UserPaths, paths: &Paths) -> Plan {
    plan_with_state_dir(
        user,
        paths,
        exvlan_control::paths::machine_id_state_dir().as_deref(),
    )
}

/// 生成计划（显式指定 `machine_id` 落点目录）。
///
/// 移动端没有 `machine_id_state_dir()` 那套三端约定（Android 的 app 目录由
/// 调用方给），所以落点必须能传进来；`None` = 当前平台无法定位。
pub fn plan_with_state_dir(user: &UserPaths, paths: &Paths, state_dir: Option<&Path>) -> Plan {
    let legacy = exvlan_control::legacy::detect(user, paths);
    let prefs = prefs::plan(&user.data);
    let (machine_id, machine_id_step) = machine_id::plan(&paths.core, state_dir);
    Plan {
        user: user.clone(),
        paths: paths.clone(),
        legacy,
        prefs,
        machine_id,
        machine_id_step,
    }
}

impl Plan {
    /// 是否有旧数据/需要改写的东西（`false` = CLI 的「退出码 1」）。
    pub fn nothing_to_do(&self) -> bool {
        self.legacy.item_labels().is_empty() && !self.prefs.changed() && self.machine_id.is_none()
    }

    /// 闸门列表（路径 + 说明），一条一行。
    pub fn items(&self) -> Vec<String> {
        self.legacy.item_labels()
    }

    /// 备份清单：**会改写的**文件 + 会被删的源。
    ///
    /// 顺序与 `docs/migrate-7.3.md` §5 一致（备份目录按**文件名**回灌）。
    /// 目录布局 / 权限 / `chown` 由宿主决定（桌面 `exvlan-migrate-backup-<ts>/`）。
    pub fn backup_items(&self) -> Vec<PathBuf> {
        let legacy = &self.legacy;
        let mut items: Vec<PathBuf> = Vec::new();
        items.extend(legacy.networks_toml.clone());
        items.extend(legacy.service_json.clone());
        items.extend(legacy.config_d.clone());
        items.extend(legacy.ui_state.iter().cloned());
        items.extend(legacy.prev_kernels.iter().cloned());
        // 会改写的 `prefs.toml` 必须先留底（回滚靠它）。
        if self.prefs.file_exists && self.prefs.changed() {
            items.push(self.prefs.path.clone());
        }
        // `service.toml` 归一（S8）也是原地改写 → 有旧形状就先留底。
        if let Some(path) = &legacy.service_toml {
            items.push(path.clone());
        }
        items
    }

    /// 执行（不备份、不清旧服务定义）：lib 测试 / 移动端 / dry-run 走查用。
    pub fn apply(&self, dry_run: bool) -> anyhow::Result<Report> {
        self.apply_with(dry_run, &mut NoHost)
    }

    /// 执行（宿主提供备份 / 旧服务定义清理 / 日志 / 属主）。
    pub fn apply_with(&self, dry_run: bool, host: &mut dyn Host) -> anyhow::Result<Report> {
        let items = self.items();
        if self.nothing_to_do() {
            return Ok(Report {
                json: json!({"ok": true, "actions": [], "note": "nothing to do"}),
                nothing_to_do: true,
                backup: None,
                incomplete: Vec::new(),
                converted_networks: 0,
                removed: Vec::new(),
            });
        }
        host.log(
            Level::Info,
            &format!("phase=detect legacy={} items={:?}", items.len(), items),
        );

        // 1) 备份（dry-run 不做；宿主决定布局 / 属主）。
        let backup = if dry_run { None } else { host.backup(self)? };

        // 2) 转换（失败 → 保留源、不写标记）。dry-run 也走一遍解析，但不落盘，
        //    这样报告里能提前看见「要转几个网络」。
        let mut converted = 0usize;
        let mut networks_step = Step::skipped("networks", "没有 networks.toml");
        let mut conversion_failed: Option<String> = None;
        if let Some(src) = &self.legacy.networks_toml {
            match convert::convert_networks(src, &self.paths.config, dry_run, host) {
                Ok(n) => {
                    converted = n;
                    networks_step = Step::new(
                        "networks",
                        if dry_run { Status::Planned } else { Status::Ok },
                    )
                    .with_detail(json!({"converted": n}));
                }
                Err(err) => {
                    let reason = format!("转换 networks.toml 失败: {err:#}");
                    host.log(Level::Error, &reason);
                    conversion_failed = Some(reason);
                    networks_step = Step::failed("networks", format!("{err:#}"));
                }
            }
        }
        // `service.toml` 已存在时**不覆盖**：可能已经迁移过/手工修过（覆盖会把用户的
        // policy / 白名单再抹一次，正是 pre.17 首个版本踩过的坑）。
        let mut service_step = Step::skipped("service_json", "没有 service.json");
        if let Some(src) = &self.legacy.service_json {
            if self.paths.service_config.is_file() {
                host.log(
                    Level::Warn,
                    &format!(
                        "{} 已存在，跳过转换（保留现值）",
                        self.paths.service_config.display()
                    ),
                );
                service_step = Step::skipped(
                    "service_json",
                    format!("{} 已存在，保留现值", self.paths.service_config.display()),
                );
            } else {
                match convert::convert_service(src, &self.paths.service_config, dry_run, host) {
                    Ok(()) => {
                        host.log(Level::Info, "convert service.json → service.toml");
                        service_step = Step::new(
                            "service_json",
                            if dry_run { Status::Planned } else { Status::Ok },
                        );
                    }
                    Err(err) => {
                        let reason = format!("转换 service.json 失败: {err:#}");
                        host.log(Level::Error, &reason);
                        conversion_failed = Some(reason);
                        service_step = Step::failed("service_json", format!("{err:#}"));
                    }
                }
            }
        }
        // 转换失败时**保留源文件**并不写标记：宁可让用户重跑，也不能「迁移到一半把源删了」。
        // dry-run 不 bail：让用户先看完整份计划。
        if let Some(err) = &conversion_failed
            && !dry_run
        {
            bail!(
                "{err}，源文件已保留（备份在 {}）",
                backup
                    .as_ref()
                    .map(|d| d.display().to_string())
                    .unwrap_or_else(|| "(none)".into())
            );
        }

        // 3) `service.toml` 旧字段名/旧词汇归一（S8）
        let service_toml_step = service_toml::normalize_step(&self.paths.service_config, dry_run);

        // 4) prefs / app_mode / 陈旧键
        let (prefs_step, prefs_subs, ui_state_ok) = self.prefs.apply(dry_run);

        // 5) machine_id（COPY，不删旧文件）
        let machine_step = match (&self.machine_id, &self.machine_id_step) {
            (Some(step_plan), _) => {
                let step = step_plan.apply(dry_run);
                // 提权运行时新目录可能是 root 建的，用户要能读自己的 machine_id。
                if let Some(parent) = step_plan.to.parent() {
                    host.adopt(parent);
                }
                if step_plan.to.exists() {
                    host.adopt(&step_plan.to);
                }
                step
            }
            (None, Some(step)) => step.clone(),
            (None, None) => Step::skipped("machine_id", "无意外的 machine_id 状态"),
        };

        // 6) 旧路径 / 旧服务定义
        //
        // 护栏：`stale_paths` 里有 `/var/lib/exvlan` 这类**可能装着旧 machine_id** 的目录。
        // machine_id 没复制成功就先别删那些目录（宁可留残留，也不能让老用户变新设备）。
        let mut stale_paths = self.legacy.stale_paths.clone();
        if let Some(plan) = &self.machine_id
            && !matches!(machine_step.status, Status::Ok | Status::Planned)
        {
            let before = stale_paths.len();
            stale_paths.retain(|path| !plan.from.starts_with(path));
            if stale_paths.len() != before {
                host.log(
                    Level::Warn,
                    &format!(
                        "machine_id 未归位，保留包含 {} 的旧目录",
                        plan.from.display()
                    ),
                );
            }
        }
        let stale_step = cleanup::stale_paths(&stale_paths, dry_run);
        let service_cleanup_step = host
            .legacy_services(&self.legacy.services, dry_run)
            .unwrap_or_else(|| {
                if self.legacy.services.is_empty() {
                    Step::skipped("service_cleanup", "没有旧服务定义")
                } else {
                    Step::skipped(
                        "service_cleanup",
                        "当前宿主不清理旧服务定义（需要桌面 CLI 的 root/管理员权限）",
                    )
                }
            });

        // 7) 删源（`ui-state.json` 只在 prefs 成功时删）
        let cleanup_step = cleanup::source_step(&self.legacy, ui_state_ok, dry_run);

        let mut steps = vec![
            prefs_step,
            service_toml_step,
            machine_step,
            machine_id::uid_hash_note(),
            networks_step,
            service_step,
            stale_step,
            service_cleanup_step,
            cleanup_step,
        ];
        steps.extend(prefs_subs);
        log_steps(host, &steps);

        let mut removed: Vec<String> = Vec::new();
        for step in steps.iter().filter(|s| s.id == "cleanup") {
            if let Some(list) = step.detail.get("removed").and_then(Value::as_array) {
                removed.extend(list.iter().filter_map(|v| v.as_str().map(str::to_string)));
            }
        }
        let incomplete: Vec<String> = steps
            .iter()
            .filter(|s| s.is_failure())
            .map(|s| s.id.to_string())
            .collect();
        let incomplete_refs: Vec<&str> = incomplete.iter().map(String::as_str).collect();

        if dry_run {
            let report = build_report(
                None,
                true,
                converted,
                &removed,
                &items,
                &steps,
                &incomplete_refs,
            );
            host.log(Level::Warn, "dry-run：未做任何修改（加 --yes 执行）");
            return Ok(Report {
                json: report,
                nothing_to_do: false,
                backup: None,
                incomplete,
                converted_networks: converted,
                removed,
            });
        }

        let report = build_report(
            backup.as_deref(),
            false,
            converted,
            &removed,
            &items,
            &steps,
            &incomplete_refs,
        );
        Ok(Report {
            json: report,
            nothing_to_do: false,
            backup,
            incomplete,
            converted_networks: converted,
            removed,
        })
    }
}

/// 各步骤的日志摘要（日志保持一行一项，便于 grep）。
fn log_steps(host: &mut dyn Host, steps: &[Step]) {
    for step in steps {
        match &step.reason {
            Some(reason) => host.log(
                Level::Info,
                &format!("step {}: {} ({})", step.id, step.status.as_str(), reason),
            ),
            None => host.log(
                Level::Info,
                &format!("step {}: {}", step.id, step.status.as_str()),
            ),
        }
    }
}

fn build_report(
    backup: Option<&Path>,
    dry_run: bool,
    converted: usize,
    removed: &[String],
    legacy_items: &[String],
    steps: &[Step],
    incomplete: &[&str],
) -> Value {
    let mut report = json!({
        "tool_version": TOOL_VERSION,
        "backend": exvlan_control::detect().as_str(),
        "ok": incomplete.is_empty(),
        "dry_run": dry_run,
        "backup": backup.map(|p| p.display().to_string()),
        "converted_networks": converted,
        "removed": removed,
        "legacy": legacy_items,
        "incomplete": incomplete,
        "ts": exvlan_control::tokens::now_secs(),
    });
    if let Value::Object(summary) = report::steps_json(steps) {
        for (key, value) in summary {
            report[key] = value;
        }
    }
    report
}

/// 写 `migrate.done.v3`（迁移完成的标记）。
///
/// 布局与内容属于契约（`--rollback` 与 GUI 闸门都看它）：
/// `{version, ts, backup, incomplete}`。
pub fn write_marker(data: &Path, backup: &Path, incomplete: &[String]) -> anyhow::Result<PathBuf> {
    let marker = data.join(MIGRATE_DONE_MARKER);
    std::fs::write(
        &marker,
        serde_json::to_string_pretty(&json!({
            "version": TOOL_VERSION,
            "ts": exvlan_control::tokens::now_secs(),
            "backup": backup.display().to_string(),
            // 有失败项时源文件仍在（ui-state.json 等），这里记下来便于人工复查。
            "incomplete": incomplete,
        }))?,
    )
    .with_context(|| format!("写 {} 失败", MIGRATE_DONE_MARKER))?;
    Ok(marker)
}
