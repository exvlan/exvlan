//! 用户网络列表：`networks.toml`。
//!
//! 以前存在 WebView `localStorage.networkList`（重启/升级可能丢）。现在由 Rust
//! 独占读写，前端通过 `get_networks` / `set_networks`（JSON 字符串）访问。
//!
//! 结构：
//! ```toml
//! schema = 1
//!
//! [[network]]
//! source = "user"
//! [network.config]
//! instance_id = "..."
//! peer_urls = ["tcp://..."]
//! ```
//!
//! protobuf-JSON 的 camelCase 键会转成 snake_case 写入 TOML，读回时再转回去；
//! `null` 直接省略（TOML 不支持）。

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
#[allow(unused_imports)]
use tauri::Manager as _;

const APP_DIR: &str = crate::paths::APP_ID;
const FILE_NAME: &str = "networks.toml";
const SCHEMA: i64 = 1;

/// Linux 的配置目录（XDG_CONFIG_HOME）。
#[cfg(target_os = "linux")]
fn linux_config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir).join(APP_DIR));
    }
    let home = std::env::var_os("HOME").filter(|v| !v.is_empty())?;
    Some(PathBuf::from(home).join(".config").join(APP_DIR))
}

pub fn networks_path(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    #[cfg(target_os = "android")]
    {
        let dir = app
            .path()
            .app_config_dir()
            .map_err(|e| format!("无法定位应用目录: {e}"))?
            .join("config");
        Ok(dir.join(FILE_NAME))
    }
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        linux_config_dir()
            .map(|d| d.join(FILE_NAME))
            .ok_or_else(|| "无法定位 $XDG_CONFIG_HOME / $HOME".to_string())
    }
    #[cfg(not(any(target_os = "android", target_os = "linux")))]
    {
        let dir = app
            .path()
            .app_config_dir()
            .map_err(|e| format!("无法定位应用配置目录: {e}"))?;
        Ok(dir.join(FILE_NAME))
    }
}

// ---------------------------------------------------------------------------
// JSON <-> TOML
// ---------------------------------------------------------------------------

/// camelCase → snake_case（pb-JSON 的键都是 lowerCamelCase）。
fn to_snake(key: &str) -> String {
    let mut out = String::with_capacity(key.len() + 4);
    for (i, ch) in key.char_indices() {
        if ch.is_ascii_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

/// snake_case → camelCase（`to_snake` 的逆）。
fn to_camel(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    let mut upper = false;
    for ch in key.chars() {
        if ch == '_' {
            upper = true;
            continue;
        }
        if upper {
            out.push(ch.to_ascii_uppercase());
            upper = false;
        } else {
            out.push(ch);
        }
    }
    out
}

fn json_to_toml(value: &Value) -> Option<toml::Value> {
    match value {
        Value::Null => None,
        Value::Bool(b) => Some(toml::Value::Boolean(*b)),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(toml::Value::Integer(i))
            } else {
                n.as_f64().map(toml::Value::Float)
            }
        }
        Value::String(s) => Some(toml::Value::String(s.clone())),
        Value::Array(items) => {
            // 数组里出现 null 时 TOML 无法表达；用空字符串占位并不安全，
            // 因此整段返回 None（调用方会退化为「保留 JSON 字符串」）。
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                let converted = json_to_toml(item)?;
                out.push(converted);
            }
            Some(toml::Value::Array(out))
        }
        Value::Object(map) => {
            let mut table = toml::map::Map::new();
            for (key, item) in map {
                if let Some(converted) = json_to_toml(item) {
                    table.insert(to_snake(key), converted);
                }
            }
            Some(toml::Value::Table(table))
        }
    }
}

fn toml_to_json(value: &toml::Value) -> Value {
    match value {
        toml::Value::String(s) => Value::String(s.clone()),
        toml::Value::Integer(i) => Value::Number((*i).into()),
        toml::Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(Value::Number)
            .unwrap_or(Value::Null),
        toml::Value::Boolean(b) => Value::Bool(*b),
        toml::Value::Datetime(dt) => Value::String(dt.to_string()),
        toml::Value::Array(items) => Value::Array(items.iter().map(toml_to_json).collect()),
        toml::Value::Table(table) => {
            let mut map = Map::new();
            for (key, item) in table {
                map.insert(to_camel(key), toml_to_json(item));
            }
            Value::Object(map)
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct NetworksFile {
    #[serde(default)]
    schema: i64,
    #[serde(default)]
    network: Vec<NetworkEntry>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct NetworkEntry {
    #[serde(default = "default_source")]
    source: String,
    /// 配置本体。正常情况下是 TOML 表；无法无损转换时退化为 JSON 字符串。
    #[serde(default = "empty_table")]
    config: toml::Value,
    /// 退化时使用（config 为字符串时的原始 JSON）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    config_json: Option<String>,
}

fn default_source() -> String {
    "user".to_string()
}

fn empty_table() -> toml::Value {
    toml::Value::Table(toml::map::Map::new())
}

/// 把前端传来的 `[{config, source}]` 转成 TOML 结构。
fn encode(networks: &Value) -> Result<NetworksFile, String> {
    let list = networks
        .as_array()
        .ok_or_else(|| "networks 必须是数组".to_string())?;
    let mut entries = Vec::with_capacity(list.len());
    for item in list {
        let source = item
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("user")
            .to_string();
        let config = item.get("config").cloned().unwrap_or(Value::Null);
        // 兼容旧格式：数组元素就是 config 本身。
        let config = if config.is_null() {
            item.clone()
        } else {
            config
        };

        match json_to_toml(&config) {
            Some(toml_value @ toml::Value::Table(_)) => entries.push(NetworkEntry {
                source,
                config: toml_value,
                config_json: None,
            }),
            _ => entries.push(NetworkEntry {
                source,
                config: toml::Value::Table(toml::map::Map::new()),
                config_json: Some(
                    serde_json::to_string(&config)
                        .map_err(|e| format!("序列化网络配置失败: {e}"))?,
                ),
            }),
        }
    }
    Ok(NetworksFile {
        schema: SCHEMA,
        network: entries,
    })
}

/// TOML → 前端期望的 `[{config, source}]`。
fn decode(file: &NetworksFile) -> Value {
    let list: Vec<Value> = file
        .network
        .iter()
        .map(|entry| {
            let config = match &entry.config_json {
                Some(raw) => serde_json::from_str(raw).unwrap_or(Value::Null),
                None => toml_to_json(&entry.config),
            };
            serde_json::json!({
                "config": config,
                "source": entry.source,
            })
        })
        .collect();
    Value::Array(list)
}

// ---------------------------------------------------------------------------
// 命令
// ---------------------------------------------------------------------------

fn read_file(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(format!("读取 {} 失败: {err}", path.display())),
    }
}

/// 旧 `localStorage` 的 `networkList` 迁移：文件不存在时调用一次。
#[tauri::command]
pub fn migrate_networks(app: tauri::AppHandle, networks: String) -> Result<bool, String> {
    let path = networks_path(&app)?;
    if path.is_file() {
        return Ok(false);
    }
    let value: Value =
        serde_json::from_str(&networks).map_err(|e| format!("网络列表不是合法 JSON: {e}"))?;
    if value.as_array().map(|a| a.is_empty()).unwrap_or(true) {
        return Ok(false);
    }
    save_value(&app, &value)?;
    crate::logger::record(
        "networks",
        "migrated localStorage networkList → networks.toml",
    );
    Ok(true)
}

fn save_value(app: &tauri::AppHandle, value: &Value) -> Result<(), String> {
    let path = networks_path(app)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("创建配置目录失败（{}）: {e}", dir.display()))?;
    }
    let file = encode(value)?;
    let text = toml::to_string_pretty(&file).map_err(|e| format!("序列化 networks 失败: {e}"))?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, &text).map_err(|e| format!("写入 {} 失败: {e}", tmp.display()))?;
    crate::paths::restrict_file(&tmp);
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("替换 {} 失败: {e}", path.display())
    })?;
    let back = std::fs::read_to_string(&path).unwrap_or_default();
    if back != text {
        return Err(format!("networks 写入校验失败（{}）", path.display()));
    }
    Ok(())
}

/// 读取网络列表（JSON 字符串，前端直接 `JSON.parse`）。
#[tauri::command]
pub fn get_networks(app: tauri::AppHandle) -> Result<String, String> {
    let path = networks_path(&app)?;
    let Some(text) = read_file(&path)? else {
        return Ok("[]".to_string());
    };
    let file: NetworksFile = toml::from_str(&text).map_err(|e| {
        crate::logger::record("networks", format!("解析 {} 失败: {e}", path.display()));
        format!("networks.toml 解析失败: {e}")
    })?;
    serde_json::to_string(&decode(&file)).map_err(|e| format!("序列化失败: {e}"))
}

/// 写入网络列表（入参是 JSON 字符串）。
#[tauri::command]
pub fn set_networks(app: tauri::AppHandle, networks: String) -> Result<(), String> {
    let value: Value =
        serde_json::from_str(&networks).map_err(|e| format!("网络列表不是合法 JSON: {e}"))?;
    save_value(&app, &value)
}

/// 网络列表文件路径（诊断用）。
#[tauri::command]
pub fn networks_path_cmd(app: tauri::AppHandle) -> Result<String, String> {
    networks_path(&app).map(|p| p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_case_round_trip() {
        for key in [
            "instanceId",
            "peerUrls",
            "enableMagicDns",
            "ipv6PublicAddrAuto",
            "networkName",
            "mtu",
        ] {
            assert_eq!(to_camel(&to_snake(key)), key, "key={key}");
        }
    }

    #[test]
    fn config_round_trip_is_lossless() {
        let config = serde_json::json!({
            "instanceId": "1467ec85-9cf8-47bc-8c06-8dc320e9b9c5",
            "networkName": "exvlan",
            "networkSecret": "secret",
            "dhcp": false,
            "virtualIpv4": "10.254.1.6/27",
            "peerUrls": ["tcp://a:11010", "udp://a:11010"],
            "enableMagicDns": true,
            "mtu": 1380,
            "listenerUrls": [],
            "nested": { "someKey": 1, "flag": false }
        });
        let list = serde_json::json!([{ "config": config.clone(), "source": "user" }]);

        let file = encode(&list).unwrap();
        let text = toml::to_string_pretty(&file).unwrap();
        let back: NetworksFile = toml::from_str(&text).unwrap();
        let decoded = decode(&back);

        assert_eq!(decoded[0]["config"], config);
        assert_eq!(decoded[0]["source"], "user");
    }

    #[test]
    fn legacy_entry_without_wrapper_is_accepted() {
        let config = serde_json::json!({ "networkName": "legacy" });
        let list = serde_json::json!([config.clone()]);
        let file = encode(&list).unwrap();
        let decoded = decode(&file);
        assert_eq!(decoded[0]["config"], config);
        assert_eq!(decoded[0]["source"], "user");
    }

    #[test]
    fn null_fields_are_dropped() {
        let config = serde_json::json!({ "networkName": "x", "optional": null });
        let list = serde_json::json!([{ "config": config, "source": "user" }]);
        let file = encode(&list).unwrap();
        let text = toml::to_string_pretty(&file).unwrap();
        assert!(!text.contains("optional"));
    }
}
