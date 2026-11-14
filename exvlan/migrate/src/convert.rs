//! 旧数据 → 新格式的转换（S6/S7 的「转换」一步）。
//!
//! - `networks.toml`（界面形态）→ `<config>/<id>.toml`（内核形态）；
//! - `service.json`（旧服务配置）→ `service.toml`。
//!
//! 映射一律用内核自己的 `NetworkConfig::gen_config()`（同一份代码，避免手写字段表）。
//! 拆成两步是为了让移动端复用：`load_networks` 只算不写（移动端要「先算后写、
//! 失败回滚」），`convert_networks` 是桌面的一步到位版本。

use std::path::Path;

use anyhow::Context as _;
use serde_json::{Value, json};

use crate::{Host, Level};

/// 一条 `networks.toml` 条目转出来的内核配置（**只算不写**）。
#[derive(Debug, Clone)]
pub struct PlannedNetwork {
    /// 旧文件里写的 `instance_id`（`None` / 空串 = 旧文件没写）。
    pub instance_id: Option<String>,
    /// `NetworkConfig::gen_config().dump()` 的结果。
    pub toml: String,
}

/// 解析 `networks.toml` 并逐条映射成内核配置正文（不落盘）。
///
/// 任一条映射失败即整体 `Err`：迁移的原则是「宁可不迁，也不能把源删掉」，
/// 半份配置比没有配置更难查。
pub fn load_networks(src: &Path) -> anyhow::Result<Vec<PlannedNetwork>> {
    use exvlan_core::config::api_input::NetworkConfigExt as _;
    use exvlan_core::config::toml::ConfigLoader as _;
    use exvlan_proto::api::manage::NetworkConfig;

    let text =
        std::fs::read_to_string(src).with_context(|| format!("读取 {} 失败", src.display()))?;
    let value: toml::Value = toml::from_str(&text).context("解析 networks.toml 失败")?;

    let networks = value
        .get("network")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut out = Vec::with_capacity(networks.len());
    for (index, item) in networks.into_iter().enumerate() {
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
                json_value["networking_method"] = json!(1);
            }
        }
        let nc: NetworkConfig = serde_json::from_value(json_value).with_context(|| {
            format!("networks.toml 里第 {index} 条 network.config 无法映射为 NetworkConfig")
        })?;
        let instance_id = nc.instance_id.clone().filter(|s| !s.is_empty());
        let toml_cfg = nc.gen_config().context("gen_config 失败（字段不合法？）")?;
        out.push(PlannedNetwork {
            instance_id,
            toml: toml_cfg.dump(),
        });
    }
    Ok(out)
}

/// `networks.toml`（界面形态）→ `<config>/<id>.toml`（内核形态）。
///
/// 返回转换的条目数。`dry_run` 只算不写（报告里能提前看见「要转几个网络」）。
pub fn convert_networks(
    src: &Path,
    config_dir: &Path,
    dry_run: bool,
    host: &mut dyn Host,
) -> anyhow::Result<usize> {
    let planned = load_networks(src)?;
    if !dry_run {
        std::fs::create_dir_all(config_dir)
            .with_context(|| format!("创建 {} 失败", config_dir.display()))?;
    }
    let mut count = 0usize;
    for item in planned {
        let id = item.instance_id.unwrap_or_else(uuid_like);
        let path = config_dir.join(format!("{id}.toml"));
        if dry_run {
            host.log(Level::Info, &format!("(dry-run) 将写入 {}", path.display()));
        } else {
            std::fs::write(&path, &item.toml)
                .with_context(|| format!("写入 {} 失败", path.display()))?;
        }
        count += 1;
    }
    Ok(count)
}

/// `service.json`（旧）→ `service.toml`（新）。
pub fn convert_service(
    src: &Path,
    dst: &Path,
    dry_run: bool,
    host: &mut dyn Host,
) -> anyhow::Result<()> {
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
        host.log(
            Level::Info,
            &format!(
                "(dry-run) 由 service.json 生成 {}（policy={policy} owner_uid={owner}）",
                dst.display()
            ),
        );
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
    host.log(
        Level::Info,
        &format!("service policy={policy} owner_uid={owner}"),
    );
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

/// 旧文件没写 `instance_id` 时的文件名（保持与 pre.17 首个版本同一形状）。
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
        out.push_str(&crate::TOOL_VERSION.replace('.', ""));
    }
    out.truncate(32);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoHost;
    use std::path::PathBuf;

    fn temp(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static SEQ: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "exvlan-migrate-conv-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    const LEGACY: &str = r#"[[network]]
name = "old"

[network.config]
instance_id = "11111111-1111-4111-a111-111111111111"
network_name = "net"
network_secret = "secret"
hostname = "h"
peer_urls = ["tcp://192.0.2.10:11010"]
"#;

    #[test]
    fn load_networks_infers_manual_method_from_peer_urls() {
        let dir = temp("load");
        let src = dir.join("networks.toml");
        std::fs::write(&src, LEGACY).unwrap();
        let planned = load_networks(&src).expect("parse");
        assert_eq!(planned.len(), 1);
        assert_eq!(
            planned[0].instance_id.as_deref(),
            Some("11111111-1111-4111-a111-111111111111")
        );
        // Manual(1) 时才允许带 peer_urls；否则 gen_config 会把节点丢掉。
        assert!(
            planned[0].toml.contains("192.0.2.10"),
            "{}",
            planned[0].toml
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn convert_writes_one_file_per_network_and_dry_run_writes_nothing() {
        let dir = temp("write");
        let src = dir.join("networks.toml");
        std::fs::write(&src, LEGACY).unwrap();
        let out = dir.join("config");

        let n = convert_networks(&src, &out, true, &mut NoHost).expect("dry");
        assert_eq!(n, 1);
        assert!(!out.exists(), "dry-run 不建目录、不写文件");

        let n = convert_networks(&src, &out, false, &mut NoHost).expect("write");
        assert_eq!(n, 1);
        assert!(
            out.join("11111111-1111-4111-a111-111111111111.toml")
                .is_file()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_entry_fails_the_whole_file() {
        let dir = temp("broken");
        let src = dir.join("networks.toml");
        std::fs::write(
            &src,
            r#"[[network]]
[network.config]
instance_id = 42
"#,
        )
        .unwrap();
        let err = load_networks(&src).unwrap_err();
        assert!(format!("{err:#}").contains("第 0 条"), "{err:#}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
