//! 一次性迁移工具（pre.17 专用）。
//!
//! 背景：pre.17 起**运行时不做任何迁移/兼容** —— 旧数据由本工具一次性清掉
//! （铁律见 `docs/decisions-7.3.md` §14.15）。
//! 本版随安装包分发、由新版 App 首次启动时弹窗调用；下一版移除随包，只保留
//! 独立工具给漏掉的机器。
//!
//! 它做的事（按执行顺序）：
//! 0. 存量清洗：`service.toml` 里的非法 `owner_sid`；
//! 1. 检测：**唯一来源** `exvlan_control::legacy::detect`（GUI 闸门/doctor 共用）；
//! 2. 备份（**目录**形式，含 manifest，零依赖）；
//! 3. `ui-state.json` → `prefs.toml` + 旧 `app_mode` 拆分 + 陈旧 `[misc]` 键清理；
//! 4. `machine_id` 归位（COPY，不删旧文件）；
//! 5. 转换：`networks.toml` → 内核 `config/<id>.toml`；`service.json` → `service.toml`；
//! 6. 清理：旧路径清单、旧服务定义、`networks.toml` / `config.d/` / `ui-state.json*`（**条件**）/ `core/*.prev`；
//! 7. 写标记 `migrate.done` + 报告 + 日志（脱敏）。
//!
//! 安全：默认 dry-run（**每一步都能被看见**）；`--yes` 才落地；失败保留现场，
//! `--rollback --backup <dir>` 可还原。报告里每一步都有 `status`
//! （`planned|ok|skipped|partial|failed`）+ `reason`。

mod cleanup;
mod machine_id;
mod prefs;
mod report;
mod service_toml;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use serde_json::{Value, json};

use report::{Status, Step, steps_json};

/// 存量清洗的标记文件名（与 `migrate.done` 同目录 = 用户数据目录）。
///
/// 与 pre.17 迁移解耦：全新安装的机器也要跑（dev.1 的脏 `owner_sid` 正是
/// 「全新安装 + `--install --tools-dir`」产生的）。
const SANITIZE_MARKER: &str = "migrate.done.v2";

/// 迁移完成的标记（有它就不再弹闸门）。
const MIGRATE_MARKER: &str = "migrate.done";

/// 展示版本：优先用 CI 注入的构建串（`<标准版本>[-pre.N][+sha]`），
/// 本地直接 cargo build 时回落到 crate 版本。
const TOOL_VERSION: &str = match option_env!("EXVLAN_BUILD_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut dry_run = true;
    let mut rollback: Option<PathBuf> = None;
    let mut log_path: Option<PathBuf> = None;
    let mut json_out = false;
    // 备份与 `migrate.done` 的属主。GUI 提权运行本工具时传当前桌面用户 uid，
    // 否则备份会是 root 所有（用户读不了自己的备份）。
    let mut owner_uid: Option<u32> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--yes" | "-y" => dry_run = false,
            "--dry-run" => dry_run = true,
            "--json" => json_out = true,
            "--rollback" => {
                i += 1;
                rollback = args.get(i).map(PathBuf::from);
            }
            "--log" => {
                i += 1;
                log_path = args.get(i).map(PathBuf::from);
            }
            "--owner-uid" => {
                i += 1;
                owner_uid = args.get(i).and_then(|v| v.parse::<u32>().ok());
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            other => {
                eprintln!("未知参数: {other}");
                print_help();
                std::process::exit(2);
            }
        }
        i += 1;
    }

    let user = exvlan_control::paths::user_paths();
    let log_file = log_path.unwrap_or_else(|| user.logs.join("migrate.log"));
    let mut log = Logger::new(&log_file);

    let result = match rollback {
        Some(dir) => rollback_from(&dir, &user, &mut log),
        None => migrate(dry_run, &user, &mut log, json_out, owner_uid),
    };
    if let Err(err) = result {
        log.error(&format!("{err:#}"));
        eprintln!("迁移失败: {err:#}");
        std::process::exit(2);
    }
    log.info("done");
}

fn print_help() {
    println!(
        r#"exvlan-migrate {TOOL_VERSION}
一次性迁移（pre.16 → pre.17）。默认只预览。

用法:
  exvlan-migrate [--dry-run] [--json] [--log <path>] [--owner-uid <uid>]   预览
  exvlan-migrate --yes                                 执行
  exvlan-migrate --rollback <backup-dir>               回滚

迁移项: prefs.toml（ui-state.json/app_mode）· machine_id · networks.toml ·
        service.json · 旧路径 · 旧服务定义 · 内核 .prev

退出码: 0 成功 / 1 无旧数据 / 2 失败"#
    );
}

struct Logger {
    path: PathBuf,
    buf: Vec<String>,
}

impl Logger {
    fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            buf: Vec::new(),
        }
    }
    fn line(&mut self, level: &str, msg: &str) {
        let ts = now_ts();
        let line = format!("{ts} [migrate] {level:<5} {msg}");
        eprintln!("{line}");
        self.buf.push(line);
        self.flush();
    }
    fn info(&mut self, msg: &str) {
        self.line("INFO", msg);
    }
    fn warn(&mut self, msg: &str) {
        self.line("WARN", msg);
    }
    fn error(&mut self, msg: &str) {
        self.line("ERROR", msg);
    }
    fn flush(&mut self) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // 追加写；失败不阻断迁移（日志是 best-effort）。
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        {
            use std::io::Write as _;
            let _ = writeln!(f, "{}", self.buf.last().cloned().unwrap_or_default());
        }
    }
}

fn now_ts() -> String {
    // 尽量少依赖：用 UNIX 秒 + 简单格式化。
    let secs = exvlan_control::tokens::now_secs();
    format!("unix:{secs}")
}

/// 各步骤的日志摘要（日志保持一行一项，便于 grep）。
fn log_steps(log: &mut Logger, steps: &[Step]) {
    for step in steps {
        match &step.reason {
            Some(reason) => log.info(&format!(
                "step {}: {} ({})",
                step.id,
                step.status.as_str(),
                reason
            )),
            None => log.info(&format!("step {}: {}", step.id, step.status.as_str())),
        }
    }
}

fn migrate(
    dry_run: bool,
    user: &exvlan_control::paths::UserPaths,
    log: &mut Logger,
    json_out: bool,
    owner_uid: Option<u32>,
) -> anyhow::Result<()> {
    let p = exvlan_control::paths::paths();
    let outcome = run(dry_run, user, &p, log, owner_uid)?;
    if outcome.nothing_to_do {
        log.info("没有旧版本数据，无需迁移");
        if json_out {
            println!("{}", outcome.report);
        }
        std::process::exit(1);
    }
    if json_out {
        println!("{}", outcome.report);
    }
    Ok(())
}

/// 一次迁移的结论（便于测试：不打印、不退出进程）。
struct Outcome {
    report: Value,
    nothing_to_do: bool,
}

fn run(
    dry_run: bool,
    user: &exvlan_control::paths::UserPaths,
    p: &exvlan_control::paths::Paths,
    log: &mut Logger,
    owner_uid: Option<u32>,
) -> anyhow::Result<Outcome> {
    // 0) 存量清洗（与「有没有旧数据」无关）：dev.1 在 macOS 把 `--tools-dir`
    //    当 SID 写进了 `service.toml`，这里一次性抹掉（有 `migrate.done.v2` 则跳过）。
    sanitize_service_config(dry_run, user, &p.service_config, log)?;

    // 1) 检测（只读）+ 各步骤的计划
    let legacy = exvlan_control::legacy::detect(user, p);
    run_detected(dry_run, user, p, &legacy, log, owner_uid)
}

/// 迁移主体（`legacy` 由调用方检测好传入 —— 单一来源仍是
/// [`exvlan_control::legacy::detect`]，测试可以据此避免触碰真实系统目录）。
fn run_detected(
    dry_run: bool,
    user: &exvlan_control::paths::UserPaths,
    p: &exvlan_control::paths::Paths,
    legacy: &exvlan_control::legacy::Legacy,
    log: &mut Logger,
    owner_uid: Option<u32>,
) -> anyhow::Result<Outcome> {
    let prefs_plan = prefs::plan(&user.data);
    let (machine_id_plan, machine_id_skip) = machine_id::plan(&p.core);
    let items = legacy.item_labels();

    let nothing_to_do = items.is_empty() && !prefs_plan.changed() && machine_id_plan.is_none();
    if nothing_to_do {
        return Ok(Outcome {
            report: json!({"ok": true, "actions": [], "note": "nothing to do"}),
            nothing_to_do: true,
        });
    }
    log.info(&format!(
        "phase=detect legacy={} items={:?}",
        items.len(),
        items
    ));

    // 2) 备份（dry-run 不做）
    let mut backup: Option<PathBuf> = None;
    if !dry_run {
        let dir = user.data.join(format!(
            "exvlan-migrate-backup-{}",
            exvlan_control::tokens::now_secs()
        ));
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("创建备份目录失败: {}", dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).ok();
            if let Some(uid) = owner_uid {
                chown_tree(&dir, uid);
            }
        }
        let mut items_to_backup: Vec<PathBuf> = Vec::new();
        items_to_backup.extend(legacy.networks_toml.clone());
        items_to_backup.extend(legacy.service_json.clone());
        items_to_backup.extend(legacy.config_d.clone());
        items_to_backup.extend(legacy.ui_state.iter().cloned());
        items_to_backup.extend(legacy.prev_kernels.iter().cloned());
        // 会改写的 `prefs.toml` 必须先留底（回滚靠它）。
        if prefs_plan.file_exists && prefs_plan.changed() {
            items_to_backup.push(prefs_plan.path.clone());
        }
        // `service.toml` 归一（S8）也是原地改写 → 有旧形状就先留底。
        if let Some(path) = &legacy.service_toml {
            items_to_backup.push(path.clone());
        }
        let mut backed = 0usize;
        for src in &items_to_backup {
            let name = src
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "item".into());
            let dst = dir.join(&name);
            let ok = if src.is_dir() {
                copy_dir(src, &dst).is_ok()
            } else {
                std::fs::copy(src, &dst).is_ok()
            };
            if ok {
                backed += 1;
            } else {
                log.warn(&format!("备份失败（跳过）: {}", src.display()));
            }
        }
        log.info(&format!(
            "phase=backup dir={} items={}",
            dir.display(),
            backed
        ));
        backup = Some(dir);
    }

    // 3) 转换（失败 → 保留源、不写标记）。dry-run 也走一遍解析，但不落盘，
    //    这样报告里能提前看见「要转几个网络」。
    let mut converted = 0usize;
    let mut networks_step = Step::skipped("networks", "没有 networks.toml");
    let mut conversion_failed: Option<String> = None;
    if let Some(src) = &legacy.networks_toml {
        match convert_networks(src, &p.config, dry_run, log) {
            Ok(n) => {
                converted = n;
                networks_step = Step::new(
                    "networks",
                    if dry_run { Status::Planned } else { Status::Ok },
                )
                .with_detail(json!({"converted": n}));
            }
            Err(err) => {
                log.error(&format!("转换 networks.toml 失败: {err:#}"));
                conversion_failed = Some(format!("转换 networks.toml 失败: {err:#}"));
                networks_step = Step::failed("networks", format!("{err:#}"));
            }
        }
    }
    // `service.toml` 已存在时**不覆盖**：可能已经迁移过/手工修过（覆盖会把用户的
    // policy / 白名单再抹一次，正是 pre.17 首个版本踩过的坑）。
    let mut service_step = Step::skipped("service_json", "没有 service.json");
    if let Some(src) = &legacy.service_json {
        if p.service_config.is_file() {
            log.warn(&format!(
                "{} 已存在，跳过转换（保留现值）",
                p.service_config.display()
            ));
            service_step = Step::skipped(
                "service_json",
                format!("{} 已存在，保留现值", p.service_config.display()),
            );
        } else {
            match convert_service(src, &p.service_config, dry_run, log) {
                Ok(()) => {
                    log.info("convert service.json → service.toml");
                    service_step = Step::new(
                        "service_json",
                        if dry_run { Status::Planned } else { Status::Ok },
                    );
                }
                Err(err) => {
                    log.error(&format!("转换 service.json 失败: {err:#}"));
                    conversion_failed = Some(format!("转换 service.json 失败: {err:#}"));
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

    // 3.5) `service.toml` 旧字段名/旧词汇归一（S8）
    let service_toml_step = service_toml::normalize_step(&p.service_config, dry_run);

    // 4) prefs / app_mode / 陈旧键
    let (prefs_step, prefs_subs, ui_state_ok) = prefs_plan.apply(dry_run);

    // 5) machine_id（COPY，不删旧文件）
    let machine_step = match (&machine_id_plan, machine_id_skip) {
        (Some(plan), _) => {
            let step = plan.apply(dry_run);
            #[cfg(unix)]
            if let Some(uid) = owner_uid {
                // 提权运行时新目录可能是 root 建的，用户要能读自己的 machine_id。
                if let Some(parent) = plan.to.parent() {
                    chown_path(parent, uid);
                }
                if plan.to.exists() {
                    chown_path(&plan.to, uid);
                }
            }
            step
        }
        (None, Some(step)) => step,
        (None, None) => Step::skipped("machine_id", "无意外的 machine_id 状态"),
    };

    // 6) 旧路径 / 旧服务定义
    //
    // 护栏：`stale_paths` 里有 `/var/lib/exvlan` 这类**可能装着旧 machine_id** 的目录。
    // machine_id 没复制成功就先别删那些目录（宁可留残留，也不能让老用户变新设备）。
    let mut stale_paths = legacy.stale_paths.clone();
    if let Some(plan) = &machine_id_plan
        && !matches!(machine_step.status, Status::Ok | Status::Planned)
    {
        let before = stale_paths.len();
        stale_paths.retain(|path| !plan.from.starts_with(path));
        if stale_paths.len() != before {
            log.warn(&format!(
                "machine_id 未归位，保留包含 {} 的旧目录",
                plan.from.display()
            ));
        }
    }
    let stale_step = cleanup::stale_paths(&stale_paths, dry_run);
    let service_cleanup_step = cleanup::legacy_services(&legacy.services, dry_run);

    // 7) 删源（`ui-state.json` 只在 prefs 成功时删）
    let cleanup_step = cleanup::source_step(&legacy, ui_state_ok, dry_run);

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
    log_steps(log, &steps);

    let mut removed: Vec<String> = Vec::new();
    for step in steps.iter().filter(|s| s.id == "cleanup") {
        if let Some(list) = step.detail.get("removed").and_then(Value::as_array) {
            removed.extend(list.iter().filter_map(|v| v.as_str().map(str::to_string)));
        }
    }
    let incomplete: Vec<&str> = steps
        .iter()
        .filter(|s| s.is_failure())
        .map(|s| s.id)
        .collect();

    if dry_run {
        let report = build_report(None, true, converted, &removed, &items, &steps, &incomplete);
        log.warn("dry-run：未做任何修改（加 --yes 执行）");
        return Ok(Outcome {
            report,
            nothing_to_do: false,
        });
    }

    // 8) 标记 + 报告
    let backup_dir = backup
        .clone()
        .context("内部错误：非 dry-run 却没有备份目录")?;
    let report = build_report(
        Some(&backup_dir),
        false,
        converted,
        &removed,
        &items,
        &steps,
        &incomplete,
    );
    std::fs::write(
        backup_dir.join("migrate-report.json"),
        serde_json::to_string_pretty(&report)?,
    )
    .ok();
    let marker = user.data.join(MIGRATE_MARKER);
    std::fs::write(
        &marker,
        serde_json::to_string_pretty(&json!({
            "version": TOOL_VERSION,
            "ts": exvlan_control::tokens::now_secs(),
            "backup": backup_dir.display().to_string(),
            // 有失败项时源文件仍在（ui-state.json 等），这里记下来便于人工复查。
            "incomplete": incomplete,
        }))?,
    )
    .context("写 migrate.done 失败")?;
    #[cfg(unix)]
    if let Some(uid) = owner_uid {
        // 备份目录在转换期间又新增了 report/migrate.log，这里统一再 chown 一次。
        chown_tree(&backup_dir, uid);
        chown_path(&marker, uid);
    }

    log.info(&format!(
        "phase=done converted={converted} removed={} backup={} incomplete={:?}",
        removed.len(),
        backup_dir.display(),
        incomplete
    ));
    Ok(Outcome {
        report,
        nothing_to_do: false,
    })
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
    if let Value::Object(summary) = steps_json(steps) {
        for (key, value) in summary {
            report[key] = value;
        }
    }
    report
}

/// 一次性清洗 `service.toml` 里的 `owner_sid`（存量修复，标记 `migrate.done.v2`）。
///
/// 非法判据与写入侧同一份（[`exvlan_control::identity::valid_owner_sid`]）：
/// 只认 `S-1-...`，空串/`--tools-dir` 这类安装参数残渣一律抹成空串。
fn sanitize_service_config(
    dry_run: bool,
    user: &exvlan_control::paths::UserPaths,
    config: &Path,
    log: &mut Logger,
) -> anyhow::Result<()> {
    let marker = user.data.join(SANITIZE_MARKER);
    if marker.is_file() {
        log.info(&format!("sanitize: {} 已存在，跳过", marker.display()));
        return Ok(());
    }
    let text = match std::fs::read_to_string(config) {
        Ok(text) => text,
        // 还没装服务：没有可清洗的，写标记免得每次启动都再读一遍。
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            log.info(&format!("sanitize: {} 不存在，无需清洗", config.display()));
            if !dry_run {
                write_sanitize_marker(&marker, false)?;
            }
            return Ok(());
        }
        Err(err) => return Err(err).with_context(|| format!("读取 {} 失败", config.display())),
    };
    let (cleaned, changed) = strip_invalid_owner_sid(&text);
    if !changed {
        log.info("sanitize: service.toml 的 owner_sid 合法，无需清洗");
        if !dry_run {
            write_sanitize_marker(&marker, false)?;
        }
        return Ok(());
    }
    if dry_run {
        log.warn(&format!(
            "sanitize: 将清空 {} 里非法的 owner_sid（加 --yes 执行）",
            config.display()
        ));
        return Ok(());
    }
    // 临时文件 + rename：服务可能正在读同一份文件（原权限 0600，属主 root）。
    let tmp = config.with_extension("toml.new");
    std::fs::write(&tmp, &cleaned).with_context(|| format!("写入 {} 失败", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600)).ok();
    }
    std::fs::rename(&tmp, config).with_context(|| format!("替换 {} 失败", config.display()))?;
    log.info(&format!(
        "sanitize: 已清空 {} 里非法的 owner_sid",
        config.display()
    ));
    write_sanitize_marker(&marker, true)?;
    Ok(())
}

fn write_sanitize_marker(marker: &Path, cleared: bool) -> anyhow::Result<()> {
    if let Some(dir) = marker.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    std::fs::write(
        marker,
        serde_json::to_string_pretty(&json!({
            "version": TOOL_VERSION,
            "ts": exvlan_control::tokens::now_secs(),
            "owner_sid_cleared": cleared,
        }))?,
    )
    .with_context(|| format!("写 {} 失败", marker.display()))
}

/// 把 `owner_sid = "<非法值>"` 换成 `owner_sid = ""`，**其余字节原样保留**
/// （注释 / 字段顺序 / 行尾都必须不动）。
///
/// 返回 `(新正文, 是否有改动)`；没有改动时正文与输入完全一致。
fn strip_invalid_owner_sid(text: &str) -> (String, bool) {
    let mut out = String::with_capacity(text.len());
    let mut changed = false;
    for line in text.split_inclusive('\n') {
        let (body, eol) = split_eol(line);
        match owner_sid_span(body) {
            // 已经是空串 = 无需清洗（否则每次都会「改动」一次，破坏幂等）。
            Some((start, end, value))
                if !value.trim().is_empty()
                    && exvlan_control::identity::valid_owner_sid(&value).is_none() =>
            {
                out.push_str(&body[..start]);
                out.push_str("\"\"");
                out.push_str(&body[end..]);
                out.push_str(eol);
                changed = true;
            }
            _ => out.push_str(line),
        }
    }
    (out, changed)
}

/// 拆行尾（`\r\n` / `\n` / 无），用于逐行改写时原样回填。
fn split_eol(line: &str) -> (&str, &str) {
    if let Some(body) = line.strip_suffix("\r\n") {
        (body, "\r\n")
    } else if let Some(body) = line.strip_suffix('\n') {
        (body, "\n")
    } else {
        (line, "")
    }
}

/// 定位一行里 `owner_sid` 的**值区间**：`(起始字节, 结束字节, 值文本)`。
///
/// 只认「整行就是 `owner_sid = ...` 这个键」（行首可以是缩进），不碰注释或
/// 其它键的值里恰好出现的同名字样。
fn owner_sid_span(line: &str) -> Option<(usize, usize, String)> {
    const KEY: &str = "owner_sid";
    let key_at = line.find(KEY)?;
    if !line[..key_at].trim().is_empty() {
        return None;
    }
    let after_key = &line[key_at + KEY.len()..];
    let after_eq = after_key.trim_start();
    let after_eq = after_eq.strip_prefix('=')?;
    let value_start = line.len() - after_eq.len();
    let rest = &line[value_start..];
    let trimmed = rest.trim_start();
    let start = value_start + (rest.len() - trimmed.len());
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.as_bytes()[0] {
        quote @ (b'"' | b'\'') => {
            let body = &trimmed[1..];
            let close = body.find(quote as char)?;
            let end = start + 1 + close + 1;
            Some((start, end, body[..close].to_string()))
        }
        // 裸值（TOML 里非法，但历史文件可能写成 `owner_sid = --tools-dir`）
        _ => {
            let len = trimmed
                .find(|c: char| c.is_whitespace() || c == '#')
                .unwrap_or(trimmed.len());
            Some((start, start + len, trimmed[..len].to_string()))
        }
    }
}

/// `networks.toml`（界面形态）→ `<config>/<id>.toml`（内核形态）。
///
/// 用内核自己的 `NetworkConfig::gen_config()` 做映射（同一份代码，避免手写字段表）。
fn convert_networks(
    src: &Path,
    config_dir: &Path,
    dry_run: bool,
    log: &mut Logger,
) -> anyhow::Result<usize> {
    use exvlan_core::config::api_input::NetworkConfigExt as _;
    use exvlan_core::config::toml::ConfigLoader as _;
    use exvlan_proto::api::manage::NetworkConfig;

    let text =
        std::fs::read_to_string(src).with_context(|| format!("读取 {} 失败", src.display()))?;
    let value: toml::Value = toml::from_str(&text).context("解析 networks.toml 失败")?;
    if !dry_run {
        std::fs::create_dir_all(config_dir)?;
    }
    let mut count = 0usize;

    let networks = value
        .get("network")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    for item in networks {
        let Some(cfg_value) = item.get("config") else {
            continue;
        };
        let mut json_value = toml_to_json(cfg_value);
        // 旧界面可能没写 `networking_method`；缺省值在 proto 里是 PublicServer(0)，
        // 会要求 `public_server_url` 合法 → 直接把 peer_urls 丢掉。这里按「有 peer_urls
        // 就是 Manual(1)」推断，避免迁移时丢节点。
        if json_value.get("networking_method").is_none()
            && json_value.get("networkingMethod").is_none()
        {
            let has_peers = json_value
                .get("peer_urls")
                .and_then(|v| v.as_array())
                .map(|a| !a.is_empty())
                .unwrap_or(false);
            if has_peers {
                json_value["networking_method"] = serde_json::json!(1);
            }
        }
        let nc: NetworkConfig = serde_json::from_value(json_value)
            .context("networks.toml 里的 network.config 无法映射为 NetworkConfig")?;
        let id = nc
            .instance_id
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(uuid_like);
        let toml_cfg = nc.gen_config().context("gen_config 失败（字段不合法？）")?;
        let dumped = toml_cfg.dump();
        let path = config_dir.join(format!("{id}.toml"));
        if dry_run {
            log.info(&format!("(dry-run) 将写入 {}", path.display()));
        } else {
            std::fs::write(&path, &dumped)
                .with_context(|| format!("写入 {} 失败", path.display()))?;
        }
        count += 1;
    }
    Ok(count)
}

/// `service.json`（旧）→ `service.toml`（新）。
fn convert_service(src: &Path, dst: &Path, dry_run: bool, log: &mut Logger) -> anyhow::Result<()> {
    let text = std::fs::read_to_string(src)?;
    let v: Value = serde_json::from_str(&text).context("解析 service.json 失败")?;
    let owner = v.get("owner_uid").and_then(Value::as_u64).unwrap_or(0);
    let mode = v.get("mode").and_then(Value::as_str).unwrap_or("Service");
    let policy = match mode.to_ascii_lowercase().as_str() {
        "normal" | "session" => "session",
        "off" | "stopped" => "stopped",
        _ => "resident",
    };
    let extra: Vec<String> = v
        .get("rpc_extra_whitelist")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    let mut out = String::new();
    out.push_str("# 由 exvlan-migrate 从 service.json 转换（pre.17）\n");
    out.push_str(&format!("owner_uid = {owner}\n"));
    out.push_str(&format!("policy = \"{policy}\"\n"));
    // 旧版本的远端直连依赖「把虚拟网段放进 RPC 白名单」；新版本默认关闭
    // （远端统一走 gateway）。迁移时显式打开，避免升级即断远端管理。
    out.push_str("# 迁移保留旧行为：RPC 白名单包含虚拟网段（远端直连仍可用）\n");
    out.push_str("rpc_mesh_whitelist = true\n");
    if !extra.is_empty() {
        let list = extra
            .iter()
            .map(|s| format!("\"{s}\""))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("rpc_extra_whitelist = [{list}]\n"));
    }
    if dry_run {
        log.info(&format!(
            "(dry-run) 由 service.json 生成 {}（policy={policy} owner_uid={owner}）",
            dst.display()
        ));
        return Ok(());
    }
    if let Some(dir) = dst.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    std::fs::write(dst, out).with_context(|| format!("写入 {} 失败", dst.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dst, std::fs::Permissions::from_mode(0o600)).ok();
    }
    log.info(&format!("service policy={policy} owner_uid={owner}"));
    Ok(())
}

fn rollback_from(
    dir: &Path,
    user: &exvlan_control::paths::UserPaths,
    log: &mut Logger,
) -> anyhow::Result<()> {
    if !dir.is_dir() {
        bail!("备份目录不存在: {}", dir.display());
    }
    let mut restored = 0usize;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "migrate-report.json" || name == "migrate.log" {
            continue;
        }
        let dst = user.data.join(name.as_ref());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &dst)?;
        } else {
            std::fs::copy(entry.path(), &dst)?;
        }
        restored += 1;
    }
    // 恢复出来的旧 service.json / ui-state.json 意味着「迁移前状态」，标记必须去掉，
    // 否则下次启动会跳过迁移。
    for marker in [MIGRATE_MARKER, SANITIZE_MARKER] {
        let _ = std::fs::remove_file(user.data.join(marker));
    }
    log.info(&format!(
        "rollback restored={restored} from {}",
        dir.display()
    ));
    Ok(())
}

/// 把路径（及其子项）的属主改成 `uid`，组沿用该用户的主组。
///
/// 只有 root 能成功；失败不阻断迁移（只是备份属主不对，属于体验问题）。
#[cfg(unix)]
fn chown_tree(path: &Path, uid: u32) {
    chown_path(path, uid);
    if path.is_dir()
        && let Ok(entries) = std::fs::read_dir(path)
    {
        for entry in entries.flatten() {
            chown_tree(&entry.path(), uid);
        }
    }
}

#[cfg(unix)]
fn chown_path(path: &Path, uid: u32) {
    use std::os::unix::ffi::OsStrExt as _;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return;
    };
    // 拿不到主组就传 -1（= 不改 gid）。
    let gid: libc::gid_t = unsafe {
        let pw = libc::getpwuid(uid);
        if pw.is_null() {
            libc::gid_t::MAX
        } else {
            (*pw).pw_gid as libc::gid_t
        }
    };
    unsafe {
        libc::chown(c.as_ptr(), uid, gid);
    }
}

#[cfg(not(unix))]
fn chown_tree(_path: &Path, _uid: u32) {}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let target = dst.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn toml_to_json(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::String(s.clone()),
        toml::Value::Integer(i) => json!(i),
        toml::Value::Float(f) => json!(f),
        toml::Value::Boolean(b) => json!(b),
        toml::Value::Datetime(d) => Value::String(d.to_string()),
        toml::Value::Array(a) => Value::Array(a.iter().map(toml_to_json).collect()),
        toml::Value::Table(t) => Value::Object(
            t.iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect(),
        ),
    }
}

fn uuid_like() -> String {
    let s = uuid_hex();
    format!(
        "{}-{}-4{}-a{}-{}",
        &s[0..8],
        &s[8..12],
        &s[13..16],
        &s[16..19],
        &s[20..32]
    )
}

fn uuid_hex() -> String {
    let mut out = String::new();
    while out.len() < 32 {
        out.push_str(&exvlan_control::tokens::now_secs().to_string());
        out.push_str(&std::process::id().to_string());
        out.push_str(&TOOL_VERSION.replace('.', ""));
    }
    out.truncate(32);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "exvlan-migrate-test-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn strips_installer_flag_but_keeps_everything_else() {
        // dev.1 真机上的原始脏值形状：macOS 的 args[2] 是 `--tools-dir`。
        let dirty = "# 注释\npolicy = \"resident\"\nowner_sid = \"--tools-dir\"\n\n[gateway]\nenabled = true\n";
        let (clean, changed) = strip_invalid_owner_sid(dirty);
        assert!(changed);
        assert_eq!(
            clean,
            "# 注释\npolicy = \"resident\"\nowner_sid = \"\"\n\n[gateway]\nenabled = true\n"
        );
        // 幂等：再跑一次不再改动
        let (again, changed) = strip_invalid_owner_sid(&clean);
        assert!(!changed);
        assert_eq!(again, clean);
    }

    #[test]
    fn leaves_valid_owner_sid_and_crlf_alone() {
        let ok = "owner_sid = \"S-1-5-21-1004336348-1177238915-682003330-1001\"\r\npolicy = \"session\"\r\n";
        let (out, changed) = strip_invalid_owner_sid(ok);
        assert!(!changed);
        assert_eq!(out, ok);

        // 行尾注释保留，只替换值；CRLF 不能变成 LF。
        let dirty = "owner_sid = \"root\" # 历史脏值\r\npolicy = \"session\"\r\n";
        let (out, changed) = strip_invalid_owner_sid(dirty);
        assert!(changed);
        assert_eq!(
            out,
            "owner_sid = \"\" # 历史脏值\r\npolicy = \"session\"\r\n"
        );
    }

    #[test]
    fn ignores_owner_sid_mentions_elsewhere() {
        // 别的键的值 / 注释里出现 owner_sid 时不能误改。
        let text = "# owner_sid = \"--tools-dir\"\nrpc_portal = \"owner_sid\"\n";
        let (out, changed) = strip_invalid_owner_sid(text);
        assert!(!changed);
        assert_eq!(out, text);
    }

    #[test]
    fn sanitize_rewrites_config_then_marks_done() {
        let user = temp("sanitize-user");
        let sys = temp("sanitize-sys");
        let config = sys.join("service.toml");
        std::fs::write(
            &config,
            "owner_sid = \"--tools-dir\"\npolicy = \"resident\"\n",
        )
        .unwrap();
        let log_path = user.join("migrate.log");

        let mut log = Logger::new(&log_path);
        sanitize_service_config(false, &user_paths(&user), &config, &mut log).expect("sanitize");

        let text = std::fs::read_to_string(&config).unwrap();
        assert_eq!(text, "owner_sid = \"\"\npolicy = \"resident\"\n");
        assert!(user.join(SANITIZE_MARKER).is_file(), "应写 migrate.done.v2");

        // 标记存在 → 不再碰文件（即使文件又被写脏）
        std::fs::write(&config, "owner_sid = \"--tools-dir\"\n").unwrap();
        let mut log = Logger::new(&log_path);
        sanitize_service_config(false, &user_paths(&user), &config, &mut log).expect("skip");
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            "owner_sid = \"--tools-dir\"\n"
        );

        let _ = std::fs::remove_dir_all(&user);
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn sanitize_dry_run_does_not_touch_file() {
        let user = temp("sanitize-dry-user");
        let sys = temp("sanitize-dry-sys");
        let config = sys.join("service.toml");
        let dirty = "owner_sid = \"--tools-dir\"\n";
        std::fs::write(&config, dirty).unwrap();

        let mut log = Logger::new(&user.join("migrate.log"));
        sanitize_service_config(true, &user_paths(&user), &config, &mut log).expect("dry-run");

        assert_eq!(std::fs::read_to_string(&config).unwrap(), dirty);
        assert!(!user.join(SANITIZE_MARKER).is_file(), "dry-run 不写标记");

        let _ = std::fs::remove_dir_all(&user);
        let _ = std::fs::remove_dir_all(&sys);
    }

    fn user_paths(dir: &Path) -> exvlan_control::paths::UserPaths {
        exvlan_control::paths::UserPaths {
            data: dir.to_path_buf(),
            logs: dir.to_path_buf(),
        }
    }

    /// 一套完全落在临时目录里的路径（绝不碰真实系统目录）。
    struct Fake {
        root: PathBuf,
    }

    impl Fake {
        fn new(tag: &str) -> Self {
            let root = temp(tag);
            Self { root }
        }
        fn user(&self) -> exvlan_control::paths::UserPaths {
            let data = self.root.join("user");
            std::fs::create_dir_all(&data).unwrap();
            exvlan_control::paths::UserPaths {
                data: data.clone(),
                logs: data,
            }
        }
        fn paths(&self) -> exvlan_control::paths::Paths {
            let config = self.root.join("system").join("config");
            let core = self.root.join("system").join("core");
            std::fs::create_dir_all(&config).unwrap();
            std::fs::create_dir_all(&core).unwrap();
            let mut p = exvlan_control::paths::paths();
            p.config = config.clone();
            p.core = core.clone();
            p.logs = self.root.join("system").join("logs");
            p.service_config = self.root.join("system").join("service.toml");
            p.service_bin = self.root.join("system").join("bin").join("exvlan-service");
            p.kernel_bin = core.join(if cfg!(windows) {
                "exvlan.exe"
            } else {
                "exvlan"
            });
            p.gateway_bin = core.join("exvlan-gateway");
            p.cli_bin = core.join("exvlan-cli");
            p
        }
    }

    /// 一台「老机器」：ui-state.json + 旧 app_mode + 旧 networks.toml。
    fn seed_legacy(user: &exvlan_control::paths::UserPaths, p: &exvlan_control::paths::Paths) {
        std::fs::write(
            user.data.join("ui-state.json"),
            serde_json::to_string(&json!({
                "lang": "en",
                "log_level": "debug",
                "last_network_instance_id": "inst-1",
                "app_mode": format!(
                    r#"{{"mode":"remote","remote_rpc_address":"10.9.9.9:{}","config_server_url":"https://cfg/x"}}"#,
                    exvlan_control::DEFAULT_RPC_PORT
                ),
                "networkList": "[]",
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(
            user.data.join("networks.toml"),
            r#"[[network]]
name = "old"

[network.config]
instance_id = "11111111-1111-4111-a111-111111111111"
network_name = "net"
network_secret = "secret"
hostname = "h"
networking_method = 1
peer_urls = ["tcp://192.0.2.10:11010"]
"#,
        )
        .unwrap();
        std::fs::write(p.core.join("exvlan.prev"), "old-kernel").unwrap();
        // 更早的 pre.17 构建写过的旧字段名 + 旧词汇（S8）
        std::fs::write(
            &p.service_config,
            "# 服务配置\nowner_uid = 0\nmode = \"normal\"\nrpc_mesh_whitelist = true\n",
        )
        .unwrap();
    }

    /// 测试里**不碰真实系统目录**：把系统级旧路径 / 旧服务定义从检测结果里摘掉
    /// （否则 `/var/lib/exvlan` 这类真实残留会被删）。
    fn detected_for_test(
        user: &exvlan_control::paths::UserPaths,
        p: &exvlan_control::paths::Paths,
    ) -> exvlan_control::legacy::Legacy {
        use exvlan_control::legacy::Kind;
        let mut legacy = exvlan_control::legacy::detect(user, p);
        legacy.stale_paths.clear();
        legacy.services.clear();
        legacy
            .items
            .retain(|item| !matches!(item.kind, Kind::StalePath | Kind::LegacyService));
        legacy
    }

    fn run_test(
        dry_run: bool,
        user: &exvlan_control::paths::UserPaths,
        p: &exvlan_control::paths::Paths,
    ) -> anyhow::Result<Outcome> {
        let legacy = detected_for_test(user, p);
        let mut log = Logger::new(&user.data.join("migrate.log"));
        run_detected(dry_run, user, p, &legacy, &mut log, None)
    }

    fn read_prefs(user: &exvlan_control::paths::UserPaths) -> toml::Table {
        toml::from_str(&std::fs::read_to_string(user.data.join("prefs.toml")).unwrap()).unwrap()
    }

    fn prefs_str<'a>(table: &'a toml::Table, section: &str, key: &str) -> Option<&'a str> {
        table.get(section)?.get(key)?.as_str()
    }

    #[test]
    fn end_to_end_migrates_prefs_and_is_idempotent() {
        let fake = Fake::new("e2e");
        let user = fake.user();
        let p = fake.paths();
        seed_legacy(&user, &p);

        let first = run_test(false, &user, &p).expect("迁移成功");
        assert!(!first.nothing_to_do);
        assert_eq!(first.report["ok"], true, "{:#}", first.report);

        // prefs 落地，ui-state.json 被删（prefs 成功 → 允许删源）
        let table = read_prefs(&user);
        assert_eq!(prefs_str(&table, "app", "lang"), Some("en"));
        assert_eq!(prefs_str(&table, "app", "log_level"), Some("debug"));
        // app_mode 拆分：runtime=service、target/remotes 合成、config_server_url 保留
        assert_eq!(prefs_str(&table, "misc", "runtime"), Some("service"));
        assert_eq!(
            prefs_str(&table, "misc", "target"),
            Some(format!("rm-legacy-10.9.9.9-{}", exvlan_control::DEFAULT_RPC_PORT).as_str())
        );
        let remotes = prefs_str(&table, "misc", "remotes")
            .unwrap_or_default()
            .to_string();
        assert!(remotes.contains(&format!(
            "rm-legacy-10.9.9.9-{}",
            exvlan_control::DEFAULT_RPC_PORT
        )));
        // 现役设置保留在瘦身后的 [app].mode 里（前端从 loadMode() 读）
        let app_mode = prefs_str(&table, "app", "mode").unwrap_or_default();
        assert!(app_mode.contains("https://cfg/x"), "{app_mode}");
        assert!(!app_mode.contains("remote_rpc_address"));
        // 陈旧 misc 键被清掉
        assert!(prefs_str(&table, "misc", "networkList").is_none());
        assert!(!user.data.join("ui-state.json").exists());
        assert!(!user.data.join("networks.toml").exists());
        assert!(!p.core.join("exvlan.prev").exists());
        // 内核配置转换产物存在
        assert!(
            p.config
                .join("11111111-1111-4111-a111-111111111111.toml")
                .is_file()
        );
        // S8：service.toml 旧字段名归一，且保留注释与其它行
        assert_eq!(
            std::fs::read_to_string(&p.service_config).unwrap(),
            "# 服务配置\nowner_uid = 0\npolicy = \"session\"\nrpc_mesh_whitelist = true\n"
        );
        // 归一成功 → 报告明确写出「别名可安全删」
        let steps = first.report["steps"].as_array().unwrap();
        assert!(
            steps.iter().any(|s| s["id"] == "service_toml"
                && s["detail"]["runtime_alias_removal"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("可删除")),
            "{:#}",
            first.report
        );
        // marker 只写一次（内容是单次运行的账）
        let marker_path = user.data.join(MIGRATE_MARKER);
        let marker_first = std::fs::read_to_string(&marker_path).unwrap();
        assert!(marker_first.contains("backup"));

        // 第二次：没有遗留 → nothing to do，且不改任何文件
        let second = run_test(false, &user, &p).expect("第二次");
        assert!(second.nothing_to_do, "{:#}", second.report);
        assert_eq!(marker_first, std::fs::read_to_string(&marker_path).unwrap());
        assert_eq!(read_prefs(&user), table, "第二次运行不能改动 prefs.toml");

        let _ = std::fs::remove_dir_all(&fake.root);
    }

    #[test]
    fn dry_run_reports_every_step_without_writing() {
        let fake = Fake::new("dry");
        let user = fake.user();
        let p = fake.paths();
        seed_legacy(&user, &p);
        let before = std::fs::read_to_string(user.data.join("ui-state.json")).unwrap();

        let outcome = run_test(true, &user, &p).expect("dry-run");
        assert!(!outcome.nothing_to_do);
        assert_eq!(outcome.report["dry_run"], true);
        // 每个 S 项都能在 dry-run 里看见状态
        let steps = outcome.report["steps"].as_array().unwrap();
        for id in [
            "prefs",
            "app_mode",
            "service_toml",
            "machine_id_uid_hash",
            "networks",
            "stale_paths",
            "service_cleanup",
            "cleanup",
        ] {
            assert!(
                steps.iter().any(|s| s["id"] == id),
                "dry-run 报告缺 {id}: {:#}",
                outcome.report
            );
        }
        assert!(
            steps
                .iter()
                .any(|s| s["id"] == "prefs" && s["status"] == "planned"),
            "{:#}",
            outcome.report
        );
        // 不落盘
        assert_eq!(
            std::fs::read_to_string(user.data.join("ui-state.json")).unwrap(),
            before
        );
        assert!(!user.data.join("prefs.toml").exists());
        assert!(!user.data.join(MIGRATE_MARKER).exists());
        assert!(user.data.join("networks.toml").is_file());
        assert!(p.core.join("exvlan.prev").is_file());
        assert!(
            !p.config
                .join("11111111-1111-4111-a111-111111111111.toml")
                .exists()
        );
        assert!(
            !user.data.join("migrate.done.v2").exists(),
            "dry-run 不写标记"
        );
        // S8 在 dry-run 只报告
        assert!(
            steps
                .iter()
                .any(|s| s["id"] == "service_toml" && s["status"] == "planned"),
            "{:#}",
            outcome.report
        );
        assert_eq!(
            std::fs::read_to_string(&p.service_config).unwrap(),
            "# 服务配置\nowner_uid = 0\nmode = \"normal\"\nrpc_mesh_whitelist = true\n"
        );

        let _ = std::fs::remove_dir_all(&fake.root);
    }

    #[test]
    fn prefs_write_failure_keeps_ui_state_but_marks_incomplete() {
        let fake = Fake::new("prefs-fail");
        let user = fake.user();
        let p = fake.paths();
        seed_legacy(&user, &p);
        // 占住 tmp 文件名 → prefs 写入必失败
        std::fs::create_dir_all(user.data.join("prefs.toml.tmp")).unwrap();

        let outcome = run_test(false, &user, &p).expect("不该 bail");
        assert_eq!(outcome.report["ok"], false, "{:#}", outcome.report);
        assert!(user.data.join("ui-state.json").is_file(), "源必须保留");
        assert!(!user.data.join("prefs.toml").exists());
        // 其余项照常完成
        assert!(!user.data.join("networks.toml").exists());
        // marker 里明确记下未完成项（避免闸门无限循环，同时留痕）
        let marker = std::fs::read_to_string(user.data.join(MIGRATE_MARKER)).unwrap();
        assert!(marker.contains("prefs"), "{marker}");

        let _ = std::fs::remove_dir_all(&fake.root);
    }
}
