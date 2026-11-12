//! 网络配置（内核 `<config-dir>/<id>.toml`）的唯一写者入口。
//!
//! 权威数据在服务的配置目录里，**服务是唯一写者**（目录归 root），所以正常路径
//! 是走 IPC。裸核（`core` 后端、没有服务）时退回到同一目录的直接读写 —— 调用方
//! （GUI / CLI）看到的是同一套语义，不需要自己判断后端。
//!
//! 禁用状态用后缀表达：`<id>.toml.disabled` —— 内核只扫 `*.toml`，自然不加载。

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::json;

use crate::backend::{self, Backend};
use crate::{ipc, paths};

const ENABLED_SUFFIX: &str = ".toml";
const DISABLED_SUFFIX: &str = ".toml.disabled";

/// 一条配置的完整内容（列表/读取都用它）。
#[derive(Debug, Clone, Deserialize)]
pub struct Saved {
    pub id: String,
    pub enabled: bool,
    pub text: String,
    #[serde(default)]
    /// 文件字节数（等于 `text.len()`；给列表展示用）。
    pub bytes: u64,
    #[serde(default)]
    /// 最后修改时间（UNIX 秒；拿不到时为 0）。
    pub modified: u64,
}

fn config_dir() -> PathBuf {
    paths::paths().config
}

/// 实例 id → 文件名。只允许 `[A-Za-z0-9._-]`，并挡住 `../` 与隐藏文件前缀
/// （服务侧有同样的校验；这里是裸核路径的最后一道）。
fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 64 {
        return Err("实例 id 长度必须在 1..=64 之间".to_string());
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Err("实例 id 只允许 [A-Za-z0-9._-]".to_string());
    }
    if id.starts_with('.') || id.contains("..") {
        return Err("实例 id 不能以 . 开头，也不能包含 ..".to_string());
    }
    Ok(())
}

/// 服务是否可用（装了 + 能应答）。不可用就退回本地文件。
fn service_ready() -> bool {
    backend::detect() == Backend::Service && ipc::request("ping", json!({})).is_ok()
}

/// 配置目录（诊断用；由服务给出，避免客户端各自猜路径）。
pub fn dir() -> Result<String, String> {
    if service_ready() {
        let resp = ipc::request("config.list", json!({}))?;
        return Ok(resp
            .get("data")
            .and_then(|d| d.get("dir"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string());
    }
    Ok(config_dir().display().to_string())
}

/// 列出配置目录里的网络（启用 + 禁用），带正文。
pub fn list() -> Result<Vec<Saved>, String> {
    if service_ready() {
        let resp = ipc::request("config.list", json!({}))?;
        // 服务一次把正文也带回来（客户端不再逐条 read）。
        return serde_json::from_value(
            resp.get("data")
                .and_then(|d| d.get("items"))
                .cloned()
                .unwrap_or_else(|| json!([])),
        )
        .map_err(|e| format!("解析 config.list 失败: {e}"));
    }
    list_local(&config_dir())
}

fn list_local(dir: &Path) -> Result<Vec<Saved>, String> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        // 目录还不存在 = 还没有任何网络配置。
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(err) => return Err(format!("读取 {} 失败: {err}", dir.display())),
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let (id, enabled) = if let Some(id) = name.strip_suffix(DISABLED_SUFFIX) {
            (id.to_string(), false)
        } else if let Some(id) = name.strip_suffix(ENABLED_SUFFIX) {
            (id.to_string(), true)
        } else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        let meta = entry.metadata().ok();
        match std::fs::read_to_string(entry.path()) {
            Ok(text) => out.push(Saved {
                id,
                enabled,
                bytes: text.len() as u64,
                text,
                modified: meta
                    .as_ref()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            }),
            // 读不了（权限/竞态）：跳过这一条，其余照常返回。
            Err(_) => continue,
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// 读一条配置：返回 (TOML 正文, 是否启用)。
pub fn read(id: &str) -> Result<Saved, String> {
    validate_id(id)?;
    if service_ready() {
        let resp = ipc::request("config.read", json!({ "id": id }))?;
        let data = resp.get("data").ok_or("config.read 没有返回 data")?;
        let text = data
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or("config.read 没有返回 text")?
            .to_string();
        let enabled = data
            .get("enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        return Ok(Saved {
            id: id.to_string(),
            enabled,
            bytes: text.len() as u64,
            text,
            modified: 0,
        });
    }
    read_local(&config_dir(), id)
}

fn read_local(dir: &Path, id: &str) -> Result<Saved, String> {
    for (suffix, enabled) in [(ENABLED_SUFFIX, true), (DISABLED_SUFFIX, false)] {
        let path = dir.join(format!("{id}{suffix}"));
        if path.is_file() {
            let text = std::fs::read_to_string(&path)
                .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
            return Ok(Saved {
                id: id.to_string(),
                enabled,
                bytes: text.len() as u64,
                text,
                modified: path
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
            });
        }
    }
    Err(format!("配置不存在: {id}"))
}

/// 原子写入一条配置（服务侧会做 TOML 校验，并删掉另一种启用/禁用形态）。
pub fn write(id: &str, text: &str, enabled: bool) -> Result<(), String> {
    validate_id(id)?;
    if service_ready() {
        return ipc::request(
            "config.write",
            json!({ "id": id, "text": text, "enabled": enabled }),
        )
        .map(|_| ());
    }
    write_local(&config_dir(), id, text, enabled)
}

fn write_local(dir: &Path, id: &str, text: &str, enabled: bool) -> Result<(), String> {
    // 与服务侧同一套最小校验：避免把「写进去也起不来」的文件留在目录里。
    let doc: toml::Value = toml::from_str(text).map_err(|e| format!("配置不是合法 TOML: {e}"))?;
    let name = doc
        .get("network_identity")
        .and_then(|v| v.get("network_name"))
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    if name.trim().is_empty() {
        return Err("缺少 [network_identity] network_name（内核无法加载该实例）".to_string());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("创建 {} 失败: {e}", dir.display()))?;
    let tmp = dir.join(format!(".{id}.toml.tmp"));
    std::fs::write(&tmp, text).map_err(|e| format!("写 {} 失败: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644));
    }
    let suffix = if enabled {
        ENABLED_SUFFIX
    } else {
        DISABLED_SUFFIX
    };
    let dst = dir.join(format!("{id}{suffix}"));
    if let Err(err) = std::fs::rename(&tmp, &dst) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("替换 {} 失败: {err}", dst.display()));
    }
    let other = dir.join(format!(
        "{id}{}",
        if enabled {
            DISABLED_SUFFIX
        } else {
            ENABLED_SUFFIX
        }
    ));
    let _ = std::fs::remove_file(other);
    Ok(())
}

/// 删除一条配置（两种形态都删）。不存在返回 `Ok(false)`。
pub fn delete(id: &str) -> Result<bool, String> {
    validate_id(id)?;
    if service_ready() {
        return match ipc::request("config.delete", json!({ "id": id })) {
            Ok(_) => Ok(true),
            // 不存在不算错误：调用方通常按「确保它不在」来用。
            Err(err) if err.starts_with("not_found") => Ok(false),
            Err(err) => Err(err),
        };
    }
    delete_local(&config_dir(), id)
}

fn delete_local(dir: &Path, id: &str) -> Result<bool, String> {
    let mut removed = false;
    for suffix in [ENABLED_SUFFIX, DISABLED_SUFFIX] {
        let path = dir.join(format!("{id}{suffix}"));
        if path.is_file() {
            std::fs::remove_file(&path)
                .map_err(|e| format!("删除 {} 失败: {e}", path.display()))?;
            removed = true;
        }
    }
    Ok(removed)
}

/// 切换启用/禁用（不重写内容，避免覆盖别处对文件的改动）。
pub fn set_enabled(id: &str, enabled: bool) -> Result<(), String> {
    let saved = read(id)?;
    if saved.enabled == enabled {
        return Ok(());
    }
    write(id, &saved.text, enabled)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CFG: &str = "hostname = \"n\"\n[network_identity]\nnetwork_name = \"exvlan\"\n";

    fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "exvlan-cfg-test-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn rejects_traversal_and_hidden_ids() {
        for bad in ["..", "../x", "a/b", ".hid", "x..y", "", &"a".repeat(65)] {
            assert!(validate_id(bad).is_err(), "{bad:?} 必须被拒绝");
        }
        assert!(validate_id("1467ec85-9cf8-47bc-8c06-8dc320e9b9c5").is_ok());
    }

    #[test]
    fn local_round_trip_and_disable() {
        let dir = temp_dir("roundtrip");
        write_local(&dir, "net1", CFG, true).expect("write");
        let saved = read_local(&dir, "net1").expect("read");
        assert!(saved.enabled);
        assert_eq!(saved.text, CFG);

        write_local(&dir, "net1", CFG, false).expect("disable");
        assert!(!dir.join("net1.toml").exists());
        let saved = read_local(&dir, "net1").expect("read disabled");
        assert!(!saved.enabled);

        let list = list_local(&dir).expect("list");
        assert_eq!(list.len(), 1);
        assert!(!list[0].enabled);

        assert!(delete_local(&dir, "net1").expect("delete"));
        assert!(!delete_local(&dir, "net1").expect("delete again"));
        assert!(list_local(&dir).expect("list empty").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn local_write_rejects_invalid_toml() {
        let dir = temp_dir("badtoml");
        assert!(write_local(&dir, "a", "not = [valid", true).is_err());
        assert!(write_local(&dir, "a", "hostname = \"x\"\n", true).is_err());
        assert!(list_local(&dir).expect("list").is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_enabled_keeps_text() {
        let dir = temp_dir("toggle");
        write_local(&dir, "net", CFG, true).expect("write");
        // set_enabled 走的是公开入口（需要服务不可用），这里直接验证本地等价路径。
        write_local(
            &dir,
            "net",
            &read_local(&dir, "net").expect("read").text,
            false,
        )
        .expect("disable");
        assert_eq!(read_local(&dir, "net").expect("read").text, CFG);
        assert!(!read_local(&dir, "net").expect("read").enabled);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
