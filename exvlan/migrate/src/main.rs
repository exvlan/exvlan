//! 一次性迁移工具（pre.17 专用）。
//!
//! 背景：pre.17 起**运行时不做任何迁移/兼容** —— 旧数据由本工具一次性清掉。
//! 本版随安装包分发、由新版 App 首次启动时弹窗调用；下一版移除随包，只保留
//! 独立工具给漏掉的机器。
//!
//! 它做四件事：
//! 1. 备份（**目录**形式，含 manifest，零依赖）；
//! 2. 转换：`networks.toml` → 内核 `config/<id>.toml`；`service.json` → `service.toml`；
//! 3. 清理：`networks.toml` / `config.d/` / `ui-state.json*` / `core/*.prev`；
//! 4. 写标记 `migrate.done` + 报告 + 日志（脱敏）。
//!
//! 安全：默认 dry-run；`--yes` 才落地；失败保留现场，`--rollback --backup <dir>` 可还原。

use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use serde_json::{Value, json};

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

/// 选出旧 `service.json`：用户目录优先，其次**系统目录**（macOS/Windows 的服务配置）。
///
/// 只有系统目录这一处是 pre.17 首个版本漏掉的 —— 结果是升级后服务配置
/// （owner_uid / policy / rpc_extra_whitelist）被重置成默认值。
fn pick_service_json(user_data: &Path, service_config: &Path) -> Option<PathBuf> {
    [
        user_data.join("service.json"),
        service_config.with_file_name("service.json"),
    ]
    .into_iter()
    .find(|path| path.is_file())
}

#[derive(Debug, Default)]
struct Legacy {
    networks_toml: Option<PathBuf>,
    service_json: Option<PathBuf>,
    config_d: Option<PathBuf>,
    ui_state: Vec<PathBuf>,
    prev_kernels: Vec<PathBuf>,
}

fn detect(user: &exvlan_control::paths::UserPaths, p: &exvlan_control::paths::Paths) -> Legacy {
    let mut l = Legacy::default();
    let n = user.data.join("networks.toml");
    if n.is_file() {
        l.networks_toml = Some(n);
    }
    // 旧 `service.json` 可能在两处：
    // - 用户目录（历史 / Linux 裸核）；
    // - **系统目录**（macOS/Windows 的服务配置，root 持有）—— pre.17 之前就漏了
    //   这一处，导致迁移后 owner_uid/policy/rpc_extra_whitelist 全被重置成默认。
    l.service_json = pick_service_json(&user.data, &p.service_config);
    let d = user.data.join("config.d");
    if d.is_dir() {
        l.config_d = Some(d);
    }
    for name in ["ui-state.json", "ui-state.json.bak"] {
        let path = user.data.join(name);
        if path.is_file() {
            l.ui_state.push(path);
        }
    }
    for name in ["exvlan.prev", "exvlan.exe.prev"] {
        let path = p.core.join(name);
        if path.is_file() {
            l.prev_kernels.push(path);
        }
    }
    l
}

fn migrate(
    dry_run: bool,
    user: &exvlan_control::paths::UserPaths,
    log: &mut Logger,
    json_out: bool,
    owner_uid: Option<u32>,
) -> anyhow::Result<()> {
    let p = exvlan_control::paths::paths();
    let legacy = detect(user, &p);
    let mut items: Vec<String> = Vec::new();
    if let Some(v) = &legacy.networks_toml {
        items.push(v.display().to_string());
    }
    if let Some(v) = &legacy.service_json {
        items.push(v.display().to_string());
    }
    if let Some(v) = &legacy.config_d {
        items.push(v.display().to_string());
    }
    for v in legacy.ui_state.iter().chain(legacy.prev_kernels.iter()) {
        items.push(v.display().to_string());
    }

    if items.is_empty() {
        log.info("没有旧版本数据，无需迁移");
        if json_out {
            println!(
                "{}",
                json!({"ok": true, "actions": [], "note": "nothing to do"})
            );
        }
        std::process::exit(1);
    }

    log.info(&format!(
        "phase=detect legacy={} items={:?}",
        items.len(),
        items
    ));

    if dry_run {
        log.warn("dry-run：未做任何修改（加 --yes 执行）");
        if json_out {
            println!(
                "{}",
                json!({"ok": true, "dry_run": true, "legacy": items, "backup": null})
            );
        }
        return Ok(());
    }

    // 1) 备份
    let backup = user.data.join(format!(
        "exvlan-migrate-backup-{}",
        exvlan_control::tokens::now_secs()
    ));
    std::fs::create_dir_all(&backup)
        .with_context(|| format!("创建备份目录失败: {}", backup.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o700)).ok();
        if let Some(uid) = owner_uid {
            chown_tree(&backup, uid);
        }
    }
    let mut backed: Vec<String> = Vec::new();
    for src in items.iter().map(PathBuf::from) {
        let name = src
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "item".into());
        let dst = backup.join(&name);
        let ok = if src.is_dir() {
            copy_dir(&src, &dst).is_ok()
        } else {
            std::fs::copy(&src, &dst).is_ok()
        };
        if ok {
            backed.push(name);
        } else {
            log.warn(&format!("备份失败（跳过）: {}", src.display()));
        }
    }
    log.info(&format!(
        "phase=backup dir={} items={}",
        backup.display(),
        backed.len()
    ));

    // 2) 转换 networks.toml → 内核 config/*.toml
    let mut converted = 0usize;
    let mut networks_ok = true;
    if let Some(src) = &legacy.networks_toml {
        match convert_networks(src, &p.config, log) {
            Ok(n) => converted = n,
            Err(err) => {
                networks_ok = false;
                log.error(&format!("转换 networks.toml 失败: {err:#}"));
            }
        }
    }

    // 3) 转换 service.json → service.toml
    //
    // 目标已存在时**不覆盖**：可能已经迁移过/手工修过（覆盖会把用户的 policy /
    // 白名单再抹一次，正是 pre.17 首个版本踩过的坑）。
    let mut service_ok = true;
    if let Some(src) = &legacy.service_json {
        if p.service_config.is_file() {
            log.warn(&format!(
                "{} 已存在，跳过转换（保留现值）",
                p.service_config.display()
            ));
        } else {
            match convert_service(src, &p.service_config, log) {
                Ok(()) => log.info("convert service.json → service.toml"),
                Err(err) => {
                    service_ok = false;
                    log.error(&format!("转换 service.json 失败: {err:#}"));
                }
            }
        }
    }

    // 转换失败时**保留源文件**并不写标记：宁可让用户重跑，也不能「迁移到一半把源删了」。
    if !networks_ok || !service_ok {
        bail!("转换失败，源文件已保留（备份在 {}）", backup.display());
    }

    // 4) 清理
    let mut removed: Vec<String> = Vec::new();
    for path in [
        legacy.networks_toml.as_ref(),
        legacy.service_json.as_ref(),
        legacy.config_d.as_ref(),
    ]
    .into_iter()
    .flatten()
    .chain(legacy.ui_state.iter())
    .chain(legacy.prev_kernels.iter())
    {
        let ok = if path.is_dir() {
            std::fs::remove_dir_all(path).is_ok()
        } else {
            std::fs::remove_file(path).is_ok()
        };
        if ok {
            removed.push(path.display().to_string());
        }
    }
    log.info(&format!("phase=cleanup removed={}", removed.len()));

    // 5) 标记 + 报告
    let report = json!({
        "tool_version": TOOL_VERSION,
        "backend": exvlan_control::detect().as_str(),
        "backup": backup.display().to_string(),
        "converted_networks": converted,
        "removed": removed,
        "ts": exvlan_control::tokens::now_secs(),
    });
    std::fs::write(
        backup.join("migrate-report.json"),
        serde_json::to_string_pretty(&report)?,
    )
    .ok();
    let marker = user.data.join("migrate.done");
    std::fs::write(
        &marker,
        serde_json::to_string_pretty(&json!({
            "version": TOOL_VERSION,
            "ts": exvlan_control::tokens::now_secs(),
            "backup": backup.display().to_string(),
        }))?,
    )
    .context("写 migrate.done 失败")?;
    #[cfg(unix)]
    if let Some(uid) = owner_uid {
        // 备份目录在转换期间又新增了 report/migrate.log，这里统一再 chown 一次。
        chown_tree(&backup, uid);
        chown_path(&marker, uid);
    }

    log.info(&format!(
        "phase=done converted={converted} removed={} backup={}",
        removed.len(),
        backup.display()
    ));
    if json_out {
        println!("{report}");
    }
    Ok(())
}

/// `networks.toml`（界面形态）→ `<config>/<id>.toml`（内核形态）。
///
/// 用内核自己的 `NetworkConfig::gen_config()` 做映射（同一份代码，避免手写字段表）。
fn convert_networks(src: &Path, config_dir: &Path, log: &mut Logger) -> anyhow::Result<usize> {
    use exvlan_core::config::api_input::NetworkConfigExt as _;
    use exvlan_core::config::toml::ConfigLoader as _;
    use exvlan_proto::api::manage::NetworkConfig;

    let text =
        std::fs::read_to_string(src).with_context(|| format!("读取 {} 失败", src.display()))?;
    let value: toml::Value = toml::from_str(&text).context("解析 networks.toml 失败")?;
    std::fs::create_dir_all(config_dir)?;
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
        std::fs::write(&path, &dumped).with_context(|| format!("写入 {} 失败", path.display()))?;
        count += 1;
        log.info(&format!("convert network id={id} → {}", path.display()));
    }
    Ok(count)
}

/// `service.json`（旧）→ `service.toml`（新）。
fn convert_service(src: &Path, dst: &Path, log: &mut Logger) -> anyhow::Result<()> {
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
    // 恢复出来的旧 service.json 意味着「迁移前状态」，标记必须去掉，否则下次启动会跳过迁移。
    let _ = std::fs::remove_file(user.data.join("migrate.done"));
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
    fn finds_service_json_in_user_dir() {
        let user = temp("user");
        let sys = temp("sys");
        std::fs::write(user.join("service.json"), "{}").unwrap();
        let picked = pick_service_json(&user, &sys.join("service.toml"));
        assert_eq!(picked, Some(user.join("service.json")));
        let _ = std::fs::remove_dir_all(&user);
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn finds_service_json_in_system_dir() {
        // pre.17 首个版本漏掉的就是这一处（macOS/Windows 的服务配置在系统目录）。
        let user = temp("user2");
        let sys = temp("sys2");
        std::fs::write(sys.join("service.json"), "{\"mode\":\"Normal\"}").unwrap();
        let picked = pick_service_json(&user, &sys.join("service.toml"));
        assert_eq!(picked, Some(sys.join("service.json")));
        let _ = std::fs::remove_dir_all(&user);
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn prefers_user_dir_when_both_exist() {
        let user = temp("user3");
        let sys = temp("sys3");
        std::fs::write(user.join("service.json"), "{}").unwrap();
        std::fs::write(sys.join("service.json"), "{}").unwrap();
        let picked = pick_service_json(&user, &sys.join("service.toml"));
        assert_eq!(picked, Some(user.join("service.json")));
        let _ = std::fs::remove_dir_all(&user);
        let _ = std::fs::remove_dir_all(&sys);
    }

    #[test]
    fn none_when_no_legacy_service_json() {
        let user = temp("user4");
        let sys = temp("sys4");
        assert_eq!(pick_service_json(&user, &sys.join("service.toml")), None);
        let _ = std::fs::remove_dir_all(&user);
        let _ = std::fs::remove_dir_all(&sys);
    }
}
