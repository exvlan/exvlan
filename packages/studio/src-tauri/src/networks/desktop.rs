//! 网络列表（前端契约）：**配置目录视图**。
//!
//! pre.17 起没有 `networks.toml` 了 —— `<config-dir>/<id>.toml` 是唯一权威
//! （内核就加载这批文件）。这里保留 `get_networks` / `set_networks` 两个命令名，
//! 让前端契约保持稳定（收发都是 `NetworkConfig` 的 JSON 列表），但语义已变成
//! 「读/写内核配置目录」，不再有第二份存储。
//!
//! 历史：pre.13–pre.16 有三份权威（前端 localStorage、`networks.toml`、内核
//! `--config-dir`），三者漂移导致「设置没生效 / 列表里有幽灵网络」。删掉中间层后，
//! 「列表里有什么」== 「内核下次启动会加载什么」。

use exvlan_rpc::exvlan_core::config::api_input::NetworkConfigExt as _;
use exvlan_rpc::exvlan_core::config::toml::{ConfigLoader as _, TomlConfig};
use exvlan_rpc::exvlan_proto::api::manage::NetworkConfig;

use crate::config_store;

/// 配置目录路径（诊断面板展示用）。
#[tauri::command]
pub fn networks_path_cmd() -> String {
    config_store::dir()
}

/// 内核 TOML → `NetworkConfig`（与 `generate_network_config` 同源）。
fn from_toml(text: &str) -> Result<NetworkConfig, String> {
    let loader = TomlConfig::new_from_str(text).map_err(|e| e.to_string())?;
    NetworkConfig::new_from_config(&loader).map_err(|e| e.to_string())
}

/// 网络列表（JSON 字符串，前端直接 `JSON.parse`）。
///
/// 形状与旧实现一致：`[{ "config": NetworkConfig, "source": "user" }]`。
#[tauri::command]
pub fn get_networks() -> Result<String, String> {
    let mut list = Vec::new();
    for saved in config_store::list()? {
        match from_toml(&saved.text) {
            Ok(config) => list.push(serde_json::json!({
                "config": config,
                "source": "user",
            })),
            // 单条坏配置不影响整体列表，但必须留痕（否则用户只看到「网络消失」）。
            Err(err) => {
                crate::logger::record("networks", format!("解析 {} 失败，已跳过: {err}", saved.id))
            }
        }
    }
    serde_json::to_string(&list).map_err(|e| format!("序列化网络列表失败: {e}"))
}

/// 写入网络列表：逐条落盘到配置目录（已存在的保留其启用/禁用状态）。
#[tauri::command]
pub fn set_networks(networks: String) -> Result<(), String> {
    let value: serde_json::Value =
        serde_json::from_str(&networks).map_err(|e| format!("网络列表不是合法 JSON: {e}"))?;
    let items = value
        .as_array()
        .ok_or_else(|| "网络列表必须是数组".to_string())?;
    for item in items {
        let config: NetworkConfig = serde_json::from_value(
            item.get("config")
                .cloned()
                .ok_or_else(|| "网络条目缺少 config".to_string())?,
        )
        .map_err(|e| format!("网络配置不是合法 NetworkConfig: {e}"))?;
        let id = config
            .instance_id
            .clone()
            .filter(|id| !id.is_empty())
            .ok_or_else(|| "网络配置缺少 instance_id".to_string())?;
        let text = config
            .gen_config()
            .map_err(|e| format!("生成内核配置失败: {e}"))?
            .dump();
        let enabled = config_store::read(&id).map(|s| s.enabled).unwrap_or(true);
        config_store::write(&id, &text, enabled)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_round_trip_keeps_instance_id() {
        let cfg: NetworkConfig = serde_json::from_value(serde_json::json!({
            "instance_id": "1467ec85-9cf8-47bc-8c06-8dc320e9b9c5",
            "network_name": "exvlan",
            "network_secret": "s",
            "virtual_ipv4": "10.254.1.6",
            "network_length": 27,
            "peer_urls": ["tcp://xvlan.era-x.cn:11010"],
            "networking_method": 1,
        }))
        .expect("NetworkConfig");
        let text = cfg.gen_config().expect("gen_config").dump();
        let back = from_toml(&text).expect("from_toml");
        assert_eq!(back.instance_id, cfg.instance_id);
    }
}
