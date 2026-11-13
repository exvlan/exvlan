//! 旧 `networks.toml` 的解析（**只用于移动端一次性转换**）。
//!
//! 旧文件是「pb-JSON 转成 TOML」的形态：键被写成 snake_case，值是 protobuf-JSON
//! 的结构。这里把它读回 `NetworkConfig`，再由调用方用内核自己的
//! `gen_config().dump()` 落成 `<config>/<id>.toml`（与桌面同一套内核配置格式）。
//!
//! 这段代码在下一个版本会随「移动端一次性转换」一起删除。

use exvlan_rpc::exvlan_proto::api::manage::NetworkConfig;
use serde_json::{Map, Value};

/// `networks.toml` 顶层：`schema = 1` + 若干 `[[network]]`。
#[derive(Debug, serde::Deserialize)]
pub struct NetworksFile {
    #[serde(default)]
    pub schema: i64,
    #[serde(default)]
    pub network: Vec<NetworkEntry>,
}

#[derive(Debug, serde::Deserialize)]
pub struct NetworkEntry {
    #[serde(default = "default_source")]
    pub source: String,
    /// 缺 `[network.config]` 时给空表（TOML 的 Value 没有 Default）。
    #[serde(default = "empty_table")]
    pub config: toml::Value,
}

fn default_source() -> String {
    "user".to_string()
}

fn empty_table() -> toml::Value {
    toml::Value::Table(toml::map::Map::new())
}

/// snake_case → camelCase（旧文件里的键是 snake_case）。
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

/// TOML 值 → JSON（键转回 camelCase，供 `NetworkConfig` 反序列化）。
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

/// 解析旧 `networks.toml`，返回其中的网络配置。
///
/// 解析失败的**单条**会被跳过（由调用方决定是否视为整体失败），但语法错误会返回 `Err`
/// —— 迁移的原则是「宁可不迁，也不能把源文件删掉」。
pub fn load_legacy(text: &str) -> Result<Vec<NetworkConfig>, String> {
    let file: NetworksFile =
        toml::from_str(text).map_err(|e| format!("解析 networks.toml 失败: {e}"))?;
    let mut out = Vec::with_capacity(file.network.len());
    for (idx, entry) in file.network.iter().enumerate() {
        let json = toml_to_json(&entry.config);
        match serde_json::from_value::<NetworkConfig>(json) {
            Ok(cfg) => out.push(cfg),
            Err(err) => {
                crate::logger::record(
                    "networks.legacy",
                    format!("第 {idx} 条网络解析失败，已跳过: {err}"),
                );
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
schema = 1

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
    fn parses_legacy_entry() {
        let cfgs = load_legacy(SAMPLE).expect("parse");
        assert_eq!(cfgs.len(), 1);
        let cfg = &cfgs[0];
        assert_eq!(
            cfg.instance_id.as_deref(),
            Some("1467ec85-9cf8-47bc-8c06-8dc320e9b9c5")
        );
        assert_eq!(cfg.network_name.as_deref(), Some("exvlan"));
        assert_eq!(cfg.virtual_ipv4.as_deref(), Some("10.254.1.6"));
        assert_eq!(
            cfg.peer_urls,
            vec!["tcp://xvlan.era-x.cn:11010".to_string()]
        );
    }

    #[test]
    fn skips_malformed_entry_but_keeps_others() {
        let text = r#"
schema = 1
[[network]]
[network.config]
instance_id = "ok"
network_name = "exvlan"
[[network]]
[network.config]
instance_id = 42
"#;
        let cfgs = load_legacy(text).expect("parse");
        assert_eq!(cfgs.len(), 1, "坏条目应被跳过");
        assert_eq!(cfgs[0].instance_id.as_deref(), Some("ok"));
    }

    #[test]
    fn rejects_broken_toml() {
        assert!(load_legacy("[[network]\n").is_err());
    }

    #[test]
    fn camel_case_round_trip_for_keys() {
        assert_eq!(to_camel("instance_id"), "instanceId");
        assert_eq!(to_camel("enable_magic_dns"), "enableMagicDns");
        // 已经 camelCase 的键保持原样
        assert_eq!(to_camel("instanceId"), "instanceId");
    }
}
