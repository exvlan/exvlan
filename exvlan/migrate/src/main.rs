//! 一次性迁移工具（pre.17 专用）—— **CLI 外壳**。
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
//! 7. 写标记 `migrate.done.v3` + 报告 + 日志（脱敏）。
//!
//! 迁移本体在 lib（`exvlan_migrate::plan` / `Plan::apply_with`）：这里只留
//! 参数解析、日志、备份目录布局、提权属主、桌面旧服务定义清理、`--json`/退出码
//! 与 `--rollback`。移动端经 `exvlan_ffi` 复用同一个 lib（见 `exvlan_migrate::mobile`）。
//!
//! 安全：默认 dry-run（**每一步都能被看见**）；`--yes` 才落地；失败保留现场，
//! `--rollback --backup <dir>` 可还原。报告里每一步都有 `status`
//! （`planned|ok|skipped|partial|failed`）+ `reason`。

mod legacy_services;

use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};

use exvlan_migrate::{
    Host, Level, MIGRATE_DONE_MARKER, Plan, Report, SANITIZE_MARKER, TOOL_VERSION, UserPaths,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut dry_run = true;
    let mut rollback: Option<PathBuf> = None;
    let mut log_path: Option<PathBuf> = None;
    let mut json_out = false;
    // 备份与 `migrate.done.v3` 的属主。GUI 提权运行本工具时传当前桌面用户 uid，
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

/// 桌面宿主：备份目录布局 / `chown` / 旧服务定义清理 / 日志。
struct CliHost<'a> {
    log: &'a mut Logger,
    owner_uid: Option<u32>,
}

impl Host for CliHost<'_> {
    fn backup(&mut self, plan: &Plan) -> anyhow::Result<Option<PathBuf>> {
        let dir = plan.user.data.join(format!(
            "exvlan-migrate-backup-{}",
            exvlan_control::tokens::now_secs()
        ));
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("创建备份目录失败: {}", dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).ok();
            if let Some(uid) = self.owner_uid {
                chown_tree(&dir, uid);
            }
        }
        let mut backed = 0usize;
        for src in plan.backup_items() {
            let name = src
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "item".into());
            let dst = dir.join(&name);
            let ok = if src.is_dir() {
                copy_dir(&src, &dst).is_ok()
            } else {
                std::fs::copy(&src, &dst).is_ok()
            };
            if ok {
                backed += 1;
            } else {
                self.log
                    .warn(&format!("备份失败（跳过）: {}", src.display()));
            }
        }
        self.log.info(&format!(
            "phase=backup dir={} items={}",
            dir.display(),
            backed
        ));
        Ok(Some(dir))
    }

    fn legacy_services(
        &mut self,
        services: &[exvlan_control::legacy::LegacyService],
        dry_run: bool,
    ) -> Option<exvlan_migrate::Step> {
        Some(legacy_services::cleanup(services, dry_run))
    }

    fn log(&mut self, level: Level, msg: &str) {
        match level {
            Level::Info => self.log.info(msg),
            Level::Warn => self.log.warn(msg),
            Level::Error => self.log.error(msg),
        }
    }

    fn adopt(&mut self, path: &Path) {
        #[cfg(unix)]
        if let Some(uid) = self.owner_uid {
            chown_path(path, uid);
        }
        #[cfg(not(unix))]
        let _ = path;
    }
}

fn migrate(
    dry_run: bool,
    user: &UserPaths,
    log: &mut Logger,
    json_out: bool,
    owner_uid: Option<u32>,
) -> anyhow::Result<()> {
    let p = exvlan_control::paths::paths();

    // 0) 存量清洗（与「有没有旧数据」无关）：dev.1 在 macOS 把 `--tools-dir`
    //    当 SID 写进了 `service.toml`，这里一次性抹掉（有 `migrate.done.v2` 则跳过）。
    {
        let mut host = CliHost { log, owner_uid };
        exvlan_migrate::sanitize::service_config(
            dry_run,
            &user.data,
            &p.service_config,
            &mut host,
        )?;
    }

    // 1) 检测（只读）+ 各步骤的计划
    let plan = exvlan_migrate::plan(user, &p);
    let mut host = CliHost { log, owner_uid };
    let report = plan.apply_with(dry_run, &mut host)?;
    if report.nothing_to_do {
        log.info("没有旧版本数据，无需迁移");
        if json_out {
            println!("{}", report.json);
        }
        std::process::exit(1);
    }
    if !dry_run {
        finish_run(&report, user, log, owner_uid)?;
    }
    if json_out {
        println!("{}", report.json);
    }
    Ok(())
}

/// 收尾：报告落进备份目录 + 写 `migrate.done.v3` + 属主交回桌面用户。
///
/// 单独抽出来是为了让测试走**同一条**收尾路径（marker 口径一旦分叉，
/// 「v2 → v3 要不要再跑一次」的判定就会跟着分叉）。
fn finish_run(
    report: &Report,
    user: &UserPaths,
    log: &mut Logger,
    owner_uid: Option<u32>,
) -> anyhow::Result<()> {
    let backup_dir = report
        .backup
        .clone()
        .context("内部错误：非 dry-run 却没有备份目录")?;
    std::fs::write(
        backup_dir.join("migrate-report.json"),
        serde_json::to_string_pretty(&report.json)?,
    )
    .ok();
    let marker = exvlan_migrate::write_marker(&user.data, &backup_dir, &report.incomplete)?;
    #[cfg(unix)]
    if let Some(uid) = owner_uid {
        // 备份目录在转换期间又新增了 report/migrate.log，这里统一再 chown 一次。
        chown_tree(&backup_dir, uid);
        chown_path(&marker, uid);
    }
    #[cfg(not(unix))]
    let _ = (&marker, owner_uid);

    log.info(&format!(
        "phase=done converted={} removed={} backup={} incomplete={:?}",
        report.converted_networks,
        report.removed.len(),
        backup_dir.display(),
        report.incomplete
    ));
    Ok(())
}

fn rollback_from(dir: &Path, user: &UserPaths, log: &mut Logger) -> anyhow::Result<()> {
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
    // 否则下次启动会跳过迁移。`migrate.done` 是 v3 之前的老标记，一并清掉。
    for marker in [MIGRATE_DONE_MARKER, SANITIZE_MARKER, "migrate.done"] {
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

#[cfg(test)]
mod tests {
    use super::*;
    use exvlan_control::legacy::Kind;
    use exvlan_migrate::machine_id;
    use serde_json::{Value, json};

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

    /// 一套完全落在临时目录里的路径（绝不碰真实系统目录）。
    struct Fake {
        root: PathBuf,
    }

    impl Fake {
        fn new(tag: &str) -> Self {
            let root = temp(tag);
            Self { root }
        }
        fn user(&self) -> UserPaths {
            let data = self.root.join("user");
            std::fs::create_dir_all(&data).unwrap();
            UserPaths {
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
    fn seed_legacy(user: &UserPaths, p: &exvlan_control::paths::Paths) {
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
    fn plan_for_test(user: &UserPaths, p: &exvlan_control::paths::Paths) -> Plan {
        let mut plan = exvlan_migrate::plan(user, p);
        plan.legacy.stale_paths.clear();
        plan.legacy.services.clear();
        plan.legacy
            .items
            .retain(|item| !matches!(item.kind, Kind::StalePath | Kind::LegacyService));
        plan
    }

    /// 一次完整运行（与 CLI 同一条路径：计划 → 宿主 → 收尾）。
    fn run_test(
        dry_run: bool,
        user: &UserPaths,
        p: &exvlan_control::paths::Paths,
    ) -> anyhow::Result<Report> {
        let plan = plan_for_test(user, p);
        let mut log = Logger::new(&user.data.join("migrate.log"));
        let report = {
            let mut host = CliHost {
                log: &mut log,
                owner_uid: None,
            };
            plan.apply_with(dry_run, &mut host)?
        };
        if !dry_run && !report.nothing_to_do {
            finish_run(&report, user, &mut log, None)?;
        }
        Ok(report)
    }

    fn read_prefs(user: &UserPaths) -> toml::Table {
        toml::from_str(&std::fs::read_to_string(user.data.join("prefs.toml")).unwrap()).unwrap()
    }

    fn prefs_str<'a>(table: &'a toml::Table, section: &str, key: &str) -> Option<&'a str> {
        table.get(section)?.get(key)?.as_str()
    }

    fn backups(user: &UserPaths) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = std::fs::read_dir(&user.data)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().starts_with("exvlan-migrate-backup-"))
                    .unwrap_or(false)
            })
            .collect();
        out.sort();
        out
    }

    #[test]
    fn end_to_end_migrates_prefs_and_is_idempotent() {
        let fake = Fake::new("e2e");
        let user = fake.user();
        let p = fake.paths();
        seed_legacy(&user, &p);

        let first = run_test(false, &user, &p).expect("迁移成功");
        assert!(!first.nothing_to_do);
        assert_eq!(first.json["ok"], true, "{:#}", first.json);

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
        let steps = first.json["steps"].as_array().unwrap();
        assert!(
            steps.iter().any(|s| s["id"] == "service_toml"
                && s["detail"]["runtime_alias_removal"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("可删除")),
            "{:#}",
            first.json
        );
        // marker 只写一次（内容是单次运行的账）
        let marker_path = user.data.join(MIGRATE_DONE_MARKER);
        let marker_first = std::fs::read_to_string(&marker_path).unwrap();
        assert!(marker_first.contains("backup"));

        // 第二次：没有遗留 → nothing to do，且不改任何文件
        let second = run_test(false, &user, &p).expect("第二次");
        assert!(second.nothing_to_do, "{:#}", second.json);
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
        assert_eq!(outcome.json["dry_run"], true);
        // 每个 S 项都能在 dry-run 里看见状态
        let steps = outcome.json["steps"].as_array().unwrap();
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
                outcome.json
            );
        }
        assert!(
            steps
                .iter()
                .any(|s| s["id"] == "prefs" && s["status"] == "planned"),
            "{:#}",
            outcome.json
        );
        // 不落盘
        assert_eq!(
            std::fs::read_to_string(user.data.join("ui-state.json")).unwrap(),
            before
        );
        assert!(!user.data.join("prefs.toml").exists());
        assert!(!user.data.join(MIGRATE_DONE_MARKER).exists());
        assert!(user.data.join("networks.toml").is_file());
        assert!(p.core.join("exvlan.prev").is_file());
        assert!(
            !p.config
                .join("11111111-1111-4111-a111-111111111111.toml")
                .exists()
        );
        assert!(
            !user.data.join(SANITIZE_MARKER).exists(),
            "dry-run 不写标记"
        );
        // S8 在 dry-run 只报告
        assert!(
            steps
                .iter()
                .any(|s| s["id"] == "service_toml" && s["status"] == "planned"),
            "{:#}",
            outcome.json
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
        assert_eq!(outcome.json["ok"], false, "{:#}", outcome.json);
        assert!(user.data.join("ui-state.json").is_file(), "源必须保留");
        assert!(!user.data.join("prefs.toml").exists());
        // 其余项照常完成
        assert!(!user.data.join("networks.toml").exists());
        // marker 里明确记下未完成项（避免闸门无限循环，同时留痕）
        let marker = std::fs::read_to_string(user.data.join(MIGRATE_DONE_MARKER)).unwrap();
        assert!(marker.contains("prefs"), "{marker}");

        let _ = std::fs::remove_dir_all(&fake.root);
    }

    /// 「只跑过 v2 的机器」：v3 标记不存在 → 闸门重新认为是「未完成」，
    /// 且已归一的机器再跑一遍**零改动**（不重复备份、不重写文件）。
    #[test]
    fn v2_marker_reruns_but_leaves_normalized_files_untouched() {
        let fake = Fake::new("v2-v3-idle");
        let user = fake.user();
        let p = fake.paths();
        seed_legacy(&user, &p);

        let first = run_test(false, &user, &p).expect("首次迁移");
        assert!(!first.nothing_to_do);
        assert!(user.data.join(MIGRATE_DONE_MARKER).is_file());

        // 回到「只跑过 v2」的状态：v3 标记没有，只有老的 `migrate.done`。
        std::fs::remove_file(user.data.join(MIGRATE_DONE_MARKER)).unwrap();
        std::fs::write(
            user.data.join("migrate.done"),
            r#"{"version":"26.4.0-old","backup":"/old"}"#,
        )
        .unwrap();
        assert!(
            !user.data.join(MIGRATE_DONE_MARKER).exists(),
            "v3 不存在 → 闸门 / doctor 都会认为迁移还没跑过（重新触发的前提）"
        );

        let prefs_before = std::fs::read_to_string(user.data.join("prefs.toml")).unwrap();
        let prefs_mtime = std::fs::metadata(user.data.join("prefs.toml"))
            .unwrap()
            .modified()
            .unwrap();
        let service_before = std::fs::read_to_string(&p.service_config).unwrap();
        let service_mtime = std::fs::metadata(&p.service_config)
            .unwrap()
            .modified()
            .unwrap();
        let backups_before = backups(&user);

        // 再跑一次：所有步骤幂等 → 无事可做
        let second = run_test(false, &user, &p).expect("重跑");
        assert!(second.nothing_to_do, "{:#}", second.json);

        assert_eq!(
            std::fs::read_to_string(user.data.join("prefs.toml")).unwrap(),
            prefs_before,
            "prefs.toml 内容不能变"
        );
        assert_eq!(
            std::fs::metadata(user.data.join("prefs.toml"))
                .unwrap()
                .modified()
                .unwrap(),
            prefs_mtime,
            "prefs.toml 不能被重写（mtime 必须不变）"
        );
        assert_eq!(
            std::fs::read_to_string(&p.service_config).unwrap(),
            service_before
        );
        assert_eq!(
            std::fs::metadata(&p.service_config)
                .unwrap()
                .modified()
                .unwrap(),
            service_mtime,
            "service.toml 不能被重写（mtime 必须不变）"
        );
        assert_eq!(backups(&user), backups_before, "无事可做时不能重复备份");

        let _ = std::fs::remove_dir_all(&fake.root);
    }

    /// 真正的残余风险：v2 跑过之后，更早的 pre.17 构建又把 `mode = …` 写回
    /// `service.toml`。v3 下闸门会再触发一次，且这次真的把它归一掉。
    #[test]
    fn v2_machine_with_rewritten_mode_is_normalized_again() {
        let fake = Fake::new("v2-v3-mode");
        let user = fake.user();
        let p = fake.paths();
        seed_legacy(&user, &p);

        run_test(false, &user, &p).expect("首次迁移");
        assert_eq!(
            std::fs::read_to_string(&p.service_config).unwrap(),
            "# 服务配置\nowner_uid = 0\npolicy = \"session\"\nrpc_mesh_whitelist = true\n"
        );
        let prefs_before = std::fs::read_to_string(user.data.join("prefs.toml")).unwrap();
        let prefs_mtime = std::fs::metadata(user.data.join("prefs.toml"))
            .unwrap()
            .modified()
            .unwrap();

        // 更早的 pre.17 构建把旧字段名写回来了；机器上只有 v2 标记。
        std::fs::write(
            &p.service_config,
            "owner_uid = 0\nmode = \"normal\"\nrpc_mesh_whitelist = true\n",
        )
        .unwrap();
        std::fs::remove_file(user.data.join(MIGRATE_DONE_MARKER)).unwrap();
        std::fs::write(user.data.join("migrate.done"), "{}").unwrap();

        // 闸门口径：detect 仍然报 ServiceToml（items 非空）→ 会提示重跑。
        let plan = plan_for_test(&user, &p);
        assert!(
            plan.items()
                .iter()
                .any(|i| i.contains(&p.service_config.display().to_string())),
            "旧字段名必须重新进闸门: {:?}",
            plan.items()
        );

        let second = run_test(false, &user, &p).expect("重跑");
        assert!(!second.nothing_to_do);
        assert_eq!(
            std::fs::read_to_string(&p.service_config).unwrap(),
            "owner_uid = 0\npolicy = \"session\"\nrpc_mesh_whitelist = true\n"
        );
        // 其它已归一的文件零改动
        assert_eq!(
            std::fs::read_to_string(user.data.join("prefs.toml")).unwrap(),
            prefs_before
        );
        assert_eq!(
            std::fs::metadata(user.data.join("prefs.toml"))
                .unwrap()
                .modified()
                .unwrap(),
            prefs_mtime
        );
        // v3 标记重新写回
        assert!(user.data.join(MIGRATE_DONE_MARKER).is_file());

        let _ = std::fs::remove_dir_all(&fake.root);
    }

    #[test]
    fn rollback_restores_files_and_drops_v3_marker() {
        let fake = Fake::new("rollback");
        let user = fake.user();
        let p = fake.paths();
        seed_legacy(&user, &p);

        run_test(false, &user, &p).expect("迁移成功");
        let backup = backups(&user).pop().expect("备份目录");
        assert!(user.data.join(MIGRATE_DONE_MARKER).is_file());

        let mut log = Logger::new(&user.data.join("migrate.log"));
        rollback_from(&backup, &user, &mut log).expect("回滚");

        assert!(user.data.join("networks.toml").is_file(), "旧文件要回来");
        assert!(!user.data.join(MIGRATE_DONE_MARKER).exists());
        assert!(!user.data.join("migrate.done").exists());

        let _ = std::fs::remove_dir_all(&fake.root);
    }

    /// 报告 JSON 顶层键的契约（`--json` 与 GUI 消费方都依赖它）。
    #[test]
    fn report_json_keeps_documented_top_level_keys() {
        let fake = Fake::new("report-shape");
        let user = fake.user();
        let p = fake.paths();
        seed_legacy(&user, &p);

        let outcome = run_test(false, &user, &p).expect("迁移成功");
        let Value::Object(map) = &outcome.json else {
            panic!("report 必须是 JSON 对象");
        };
        for key in [
            "tool_version",
            "backend",
            "ok",
            "dry_run",
            "backup",
            "converted_networks",
            "removed",
            "legacy",
            "incomplete",
            "ts",
            "steps",
            "completed",
            "skipped",
        ] {
            assert!(
                map.contains_key(key),
                "报告缺顶层键 {key}: {:#}",
                outcome.json
            );
        }
        assert_eq!(map["tool_version"], TOOL_VERSION);
        assert_eq!(map["converted_networks"], 1);

        let _ = std::fs::remove_dir_all(&fake.root);
    }

    /// machine_id 落点必须由调用方显式给出（移动端复用同一份计划）。
    #[test]
    fn machine_id_plan_takes_explicit_state_dir() {
        let root = temp("mid-state");
        let core = root.join("core");
        std::fs::create_dir_all(&core).unwrap();
        let state = root.join("state");
        let id = "11111111-2222-3333-4444-555555555555";
        std::fs::write(core.join("et_machine_id"), id).unwrap();

        let (plan, step) = machine_id::plan(&core, Some(&state));
        assert!(step.is_none(), "{step:?}");
        let plan = plan.expect("应从 core/et_machine_id 复制");
        assert_eq!(plan.to, state.join("machine_id"));

        // 传 None = 平台无法定位（不 panic、不猜）
        let (plan, step) = machine_id::plan(&core, None);
        assert!(plan.is_none());
        assert_eq!(
            step.map(|s| s.status),
            Some(exvlan_migrate::Status::Skipped)
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
