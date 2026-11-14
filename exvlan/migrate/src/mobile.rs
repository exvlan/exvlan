//! 移动端一次性迁移：旧 `<config_dir>/networks.toml` → `<config_dir>/<id>.toml`。
//!
//! 桌面有 `exvlan-migrate` 二进制，Android 跑不了；决策是把迁移核心抽成 lib，
//! 经现有 `exvlan_ffi` 由 App 首启调用一次（见 `docs/decisions-7.3.md` §14.15）。
//!
//! 语义与桌面一致：
//! - **一次性**：`<app_config_dir>/networks-migrated` 只作观测标记，闸门是
//!   「旧文件还在不在」—— 换了恢复的备份回来仍会被处理；
//! - **失败保留源**：任一条映射 / 写入失败 → 回滚已写文件、**不删源**、不写标记，
//!   下次调用重试（等价于桌面的 dry-run「先算后写」）；
//! - **幂等**：没有旧文件 = 无事可做，不写任何东西。
//!
//! 旧文件格式与桌面 `networks.toml` 同一形状（`schema` + `[[network]]`
//! `[network.config]`），映射复用 [`crate::convert::load_networks`]（内核
//! `gen_config()`，与桌面同一份代码）。

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::convert;

/// 旧移动端把列表写在 `<app_config_dir>/config/networks.toml`
/// （与新的 `<id>.toml` **同一个目录**，不是 app 根目录）。
pub const LEGACY_FILE: &str = "networks.toml";

///「转换完成」标记（**原子性锚点**：只有全部成功才写；只用于观测，不作闸门）。
pub const MARKER: &str = "networks-migrated";

/// 启用态后缀（与 `networks/mobile.rs` 的存储约定一致）。
const ENABLED_SUFFIX: &str = ".toml";

/// 实例 id → 文件名。与服务/桌面同一套白名单（挡路径穿越）。
fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 64 {
        return Err("实例 id 长度必须在 1..=64 之间".into());
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err("实例 id 只允许 [A-Za-z0-9._-]".into());
    }
    if id.starts_with('.') || id.contains("..") {
        return Err("实例 id 不能以 . 开头，也不能包含 ..".into());
    }
    Ok(())
}

/// 原子写：`.tmp` → rename（与 `networks/mobile.rs::write_in` 同一套约定）。
fn write_atomically(dir: &Path, id: &str, text: &str) -> Result<PathBuf, String> {
    validate_id(id)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("创建 {} 失败: {e}", dir.display()))?;
    let tmp = dir.join(format!(".{id}.toml.tmp"));
    std::fs::write(&tmp, text).map_err(|e| format!("写 {} 失败: {e}", tmp.display()))?;
    let dst = dir.join(format!("{id}{ENABLED_SUFFIX}"));
    if let Err(err) = std::fs::rename(&tmp, &dst) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("替换 {} 失败: {err}", dst.display()));
    }
    Ok(dst)
}

/// 回滚一条已写文件：原来有内容就写回，原来没有就删掉。
fn rollback_one(path: &Path, previous: Option<&str>) {
    match previous {
        Some(text) => {
            let _ = std::fs::write(path, text);
        }
        None => {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// 一次性转换。返回给 FFI 的 JSON（脱敏：只有路径的**文件名**计数，不含用户数据）。
///
/// - `Ok(json)`：`{"status":"nothing_to_do"|"converted", "converted": n, ...}`
/// - `Err(msg)`：失败原因；**源文件已保留**、已写文件已回滚、未写标记。
pub fn migrate_networks_once(app_config_dir: &Path, config_dir: &Path) -> Result<Value, String> {
    let legacy_path = config_dir.join(LEGACY_FILE);
    if !legacy_path.is_file() {
        return Ok(json!({
            "status": "nothing_to_do",
            "converted": 0,
            "reason": format!("{} 不存在", legacy_path.display()),
        }));
    }

    // 1) 先算（不落盘）：解析失败 / 任一条映射失败都保持源不动。
    let planned = convert::load_networks(&legacy_path).map_err(|e| format!("{e:#}"))?;

    // 2) 后写：逐条原子写；失败就回滚已写的部分。
    let mut written: Vec<(PathBuf, Option<String>)> = Vec::new();
    let mut converted = 0usize;
    for (index, item) in planned.iter().enumerate() {
        // 缺 id 的条目在旧移动端被静默跳过 = 丢数据；这里按失败处理（源保留，可人工修）。
        let id = match item.instance_id.as_deref() {
            Some(id) if !id.is_empty() => id,
            _ => {
                rollback(&written);
                return Err(format!(
                    "{} 里第 {index} 条缺少 instance_id（源文件已保留）",
                    legacy_path.display()
                ));
            }
        };
        let previous =
            std::fs::read_to_string(config_dir.join(format!("{id}{ENABLED_SUFFIX}"))).ok();
        match write_atomically(config_dir, id, &item.toml) {
            Ok(path) => {
                written.push((path, previous));
                converted += 1;
            }
            Err(err) => {
                rollback(&written);
                return Err(format!("{id}: {err}（源文件已保留）"));
            }
        }
    }

    // 3) 全部成功才删源 + 写标记（标记失败不阻断：源已转换完，下次不再有事可做）。
    std::fs::remove_file(&legacy_path)
        .map_err(|e| format!("删除 {} 失败: {e}", legacy_path.display()))?;
    let marker = app_config_dir.join(MARKER);
    let marker_written = std::fs::write(&marker, format!("converted={converted}\n").as_bytes())
        .map(|()| true)
        .unwrap_or(false);

    Ok(json!({
        "status": "converted",
        "converted": converted,
        "removed_source": true,
        "marker": marker_written,
    }))
}

fn rollback(written: &[(PathBuf, Option<String>)]) {
    for (path, previous) in written.iter().rev() {
        rollback_one(path, previous.as_deref());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "exvlan-migrate-mobile-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    const LEGACY: &str = r#"schema = 1

[[network]]
source = "user"

[network.config]
instance_id = "1467ec85-9cf8-47bc-8c06-8dc320e9b9c5"
network_name = "exvlan"
network_secret = "s3cret"
virtual_ipv4 = "10.254.1.6"
network_length = 27
peer_urls = ["tcp://xvlan.era-x.cn:11010"]
networking_method = 1
"#;

    #[test]
    fn converts_once_and_writes_marker() {
        let root = temp("once");
        let app = root.join("app");
        let config = app.join("config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(config.join(LEGACY_FILE), LEGACY).unwrap();

        let out = migrate_networks_once(&app, &config).expect("convert");
        assert_eq!(out["status"], "converted", "{out}");
        assert_eq!(out["converted"], 1);
        let path = config.join("1467ec85-9cf8-47bc-8c06-8dc320e9b9c5.toml");
        assert!(path.is_file());
        assert!(!config.join(LEGACY_FILE).exists(), "成功后删源");
        assert!(app.join(MARKER).is_file());

        // 幂等：第二次无事可做，且不改任何文件
        let before = std::fs::read_to_string(&path).unwrap();
        let again = migrate_networks_once(&app, &config).expect("idempotent");
        assert_eq!(again["status"], "nothing_to_do", "{again}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn broken_entry_keeps_source_and_rolls_back() {
        let root = temp("rollback");
        let app = root.join("app");
        let config = app.join("config");
        std::fs::create_dir_all(&config).unwrap();
        // 第 0 条合法、第 1 条 id 非法（含 `/`）→ 第 0 条必须被回滚。
        std::fs::write(
            config.join(LEGACY_FILE),
            format!(
                "{LEGACY}
[[network]]
[network.config]
instance_id = \"bad/id\"
network_name = \"x\"
"
            ),
        )
        .unwrap();

        let err = migrate_networks_once(&app, &config).unwrap_err();
        assert!(err.contains("bad/id"), "{err}");
        assert!(config.join(LEGACY_FILE).is_file(), "失败必须保留源");
        assert!(
            !config
                .join("1467ec85-9cf8-47bc-8c06-8dc320e9b9c5.toml")
                .exists(),
            "已写文件必须回滚（否则下次重启列表里会多出一个网络）"
        );
        assert!(!app.join(MARKER).exists(), "失败不写标记");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_instance_id_is_a_failure_not_a_silent_loss() {
        let root = temp("no-id");
        let app = root.join("app");
        let config = app.join("config");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join(LEGACY_FILE),
            r#"[[network]]
[network.config]
network_name = "x"
peer_urls = ["tcp://192.0.2.10:11010"]
"#,
        )
        .unwrap();
        let err = migrate_networks_once(&app, &config).unwrap_err();
        assert!(err.contains("instance_id"), "{err}");
        assert!(config.join(LEGACY_FILE).is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn re_run_overwrites_existing_file_and_rolls_back_to_previous_content() {
        let root = temp("prev");
        let app = root.join("app");
        let config = app.join("config");
        std::fs::create_dir_all(&config).unwrap();
        let existing = config.join("1467ec85-9cf8-47bc-8c06-8dc320e9b9c5.toml");
        std::fs::write(&existing, "previous").unwrap();
        std::fs::write(
            config.join(LEGACY_FILE),
            format!(
                "{LEGACY}
[[network]]
[network.config]
instance_id = \"bad/id\"
network_name = \"x\"
"
            ),
        )
        .unwrap();

        migrate_networks_once(&app, &config).unwrap_err();
        assert_eq!(
            std::fs::read_to_string(&existing).unwrap(),
            "previous",
            "回滚必须还原覆盖前的内容，不能直接删掉用户配置"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn nothing_to_do_without_legacy_file() {
        let root = temp("empty");
        let app = root.join("app");
        let config = app.join("nope");
        let out = migrate_networks_once(&app, &config).expect("ok");
        assert_eq!(out["status"], "nothing_to_do", "{out}");
        assert!(!app.join(MARKER).exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
