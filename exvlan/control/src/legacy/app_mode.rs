//! 7.2 的 `app_mode` → 7.3 的 `runtime` × `target` × `remotes` 拆分。
//!
//! 纯逻辑（不碰文件），唯一的消费方是 `exvlan-migrate`；判定规则与前端
//! `packages/studio/src/composables/target.ts` 的 `migrateLegacyMode()` /
//! `parseAddress()` **逐字一致** —— 不一致的直接后果是老用户丢远端列表
//! （合成的 `remotes` id 与前端算出来的对不上，`target` 指向一个不存在的 id）。
//!
//! 这里不是"兼容读取"，而是一次性迁移的**规则表**：7.3 之后 `app_mode` 不再承载
//! 运行方式（见 `docs/decisions-7.3.md` §14.15）。

use serde::Serialize;

/// 旧 `app_mode` 里被 `target.ts` 消费掉的键；拆完之后只保留 `config_server_url`。
const CONSUMED_KEYS: &[&str] = &[
    "mode",
    "remote_rpc_address",
    "config_dir",
    "rpc_portal",
    "file_log_dir",
    "file_log_level",
];

/// 拆分结果里真正要落盘的远端目标（字段与前端 `RemoteTarget` 一致）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LegacyRemote {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
}

/// 一次成功的拆分。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppModeSplit {
    /// `service` / `normal`（旧 `remote` 落成 `service`：本机内核照跑，界面连别处）。
    pub runtime: &'static str,
    /// 远端目标 id（仅 `remote` 且地址合法时存在）。
    pub target: Option<String>,
    pub remotes: Vec<LegacyRemote>,
    /// 瘦身后的 `[app].mode` JSON（只剩现役设置 `config_server_url`）；
    /// `None` = 没有需要保留的键，整个键可以删掉。
    pub keep: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppModeOutcome {
    /// 解析成功，可以拆分。
    Splittable(AppModeSplit),
    /// 解析失败（JSON 坏 / `mode` 不是已知值）：**保留原值**，前端仍能工作。
    Invalid(String),
}

/// 解析 `[app].mode` 里的旧 JSON。
pub fn split(raw: &str) -> AppModeOutcome {
    let value: serde_json::Value = match serde_json::from_str(raw) {
        Ok(value) => value,
        Err(err) => return AppModeOutcome::Invalid(format!("JSON 解析失败: {err}")),
    };
    let Some(obj) = value.as_object() else {
        return AppModeOutcome::Invalid("不是 JSON 对象".into());
    };
    let keep = obj
        .get("config_server_url")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(|url| serde_json::json!({"config_server_url": url}).to_string());

    let mode = obj.get("mode").and_then(|v| v.as_str());
    match mode {
        Some("service") => AppModeOutcome::Splittable(AppModeSplit {
            runtime: "service",
            target: None,
            remotes: Vec::new(),
            keep,
        }),
        Some("normal") => AppModeOutcome::Splittable(AppModeSplit {
            runtime: "normal",
            target: None,
            remotes: Vec::new(),
            keep,
        }),
        Some("remote") => {
            let address = obj
                .get("remote_rpc_address")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            // 地址无效：保留「本机服务模式」，不让用户启动后无处可连（与前端一致）。
            let Some((host, port)) = parse_address(address) else {
                return AppModeOutcome::Splittable(AppModeSplit {
                    runtime: "service",
                    target: None,
                    remotes: Vec::new(),
                    keep,
                });
            };
            let remote = LegacyRemote {
                id: format!("rm-legacy-{host}-{port}"),
                name: format!("{host}:{port}"),
                host,
                port,
            };
            AppModeOutcome::Splittable(AppModeSplit {
                runtime: "service",
                target: Some(remote.id.clone()),
                remotes: vec![remote],
                keep,
            })
        }
        Some(other) => AppModeOutcome::Invalid(format!("未知 mode: {other}")),
        None => AppModeOutcome::Invalid("缺少 mode 字段".into()),
    }
}

/// 旧 JSON 里是否还有被本次拆分消费掉的键（没有就不必改写）。
pub fn has_consumed_keys(raw: &str) -> bool {
    let Ok(serde_json::Value::Object(obj)) = serde_json::from_str::<serde_json::Value>(raw) else {
        return false;
    };
    CONSUMED_KEYS.iter().any(|key| obj.contains_key(*key))
}

/// `host:port` / `tcp://host:port` / 纯 host 的解析（`target.ts::parseAddress` 的等价物）。
///
/// 端口缺省取 [`crate::DEFAULT_RPC_PORT`]；非法端口（0 / >65535 / 非数字）返回 `None`。
pub fn parse_address(input: &str) -> Option<(String, u16)> {
    let mut text = input.trim();
    if text.is_empty() {
        return None;
    }
    // 去掉 `tcp://` 前缀与路径部分（`tcp://host:1995/foo` 是历史粘贴格式）。
    if text.len() >= 6 && text[..6].eq_ignore_ascii_case("tcp://") {
        text = &text[6..];
    }
    if let Some(idx) = text.find('/') {
        text = &text[..idx];
    }
    if text.is_empty() {
        return None;
    }
    // [::1]:1995 / [::1]
    if let Some(rest) = text.strip_prefix('[')
        && let Some((host, tail)) = rest.split_once(']')
    {
        let port = match tail.strip_prefix(':') {
            None | Some("") => crate::DEFAULT_RPC_PORT,
            Some(raw) => raw.parse::<u16>().ok().filter(|p| *p > 0)?,
        };
        return Some((host.to_string(), port));
    }
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() == 1 {
        return Some((parts[0].to_string(), crate::DEFAULT_RPC_PORT));
    }
    let port = parts[parts.len() - 1]
        .parse::<u16>()
        .ok()
        .filter(|p| *p > 0)?;
    let host = parts[..parts.len() - 1].join(":");
    Some((host, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote_of(raw: &str) -> (AppModeSplit, String) {
        match split(raw) {
            AppModeOutcome::Splittable(split) => {
                let remotes = serde_json::to_string(&split.remotes).unwrap();
                (split, remotes)
            }
            other => panic!("应可拆分: {other:?}"),
        }
    }

    #[test]
    fn service_mode_keeps_config_server_url_only() {
        let raw = format!(
            r#"{{"mode":"service","config_dir":"/old/config","rpc_portal":"127.0.0.1:{}","file_log_dir":"/old/logs","file_log_level":"debug","config_server_url":"https://cfg.example/x"}}"#,
            crate::DEFAULT_RPC_PORT
        );
        let (split, remotes) = remote_of(&raw);
        assert_eq!(split.runtime, "service");
        assert_eq!(split.target, None);
        assert!(split.remotes.is_empty());
        assert_eq!(remotes, "[]");
        // 现役设置保留（前端从 `loadMode().config_server_url` 读）。
        assert_eq!(
            split.keep.as_deref(),
            Some(r#"{"config_server_url":"https://cfg.example/x"}"#)
        );
        assert!(has_consumed_keys(&raw));
    }

    #[test]
    fn normal_mode_without_keep_drops_key() {
        let (split, _) = remote_of(r#"{"mode":"normal","rpc_portal":"127.0.0.1:1"}"#);
        assert_eq!(split.runtime, "normal");
        assert!(split.keep.is_none());
    }

    /// `rm-legacy-<host>-<port>` 与 `target.ts::migrateLegacyMode()` 的合成结果逐字一致。
    #[test]
    fn remote_mode_synthesizes_legacy_remote_like_target_ts() {
        let raw = format!(
            r#"{{"mode":"remote","remote_rpc_address":"10.254.1.6:{}"}}"#,
            crate::DEFAULT_RPC_PORT
        );
        let (split, remotes) = remote_of(&raw);
        assert_eq!(split.runtime, "service");
        assert_eq!(
            split.target.as_deref(),
            Some(format!("rm-legacy-10.254.1.6-{}", crate::DEFAULT_RPC_PORT).as_str())
        );
        assert_eq!(
            remotes,
            format!(
                r#"[{{"id":"rm-legacy-10.254.1.6-{p}","name":"10.254.1.6:{p}","host":"10.254.1.6","port":{p}}}]"#,
                p = crate::DEFAULT_RPC_PORT
            )
        );
        // 与前端 `RemoteTarget` 字段名一致（少写 mode = direct）。
        assert!(!remotes.contains("\"mode\""));
    }

    #[test]
    fn remote_mode_accepts_explicit_port_and_tcp_prefix() {
        let (split, remotes) =
            remote_of(r#"{"mode":"remote","remote_rpc_address":"tcp://host.example:2000/x"}"#);
        assert_eq!(split.target.as_deref(), Some("rm-legacy-host.example-2000"));
        assert!(remotes.contains(r#""port":2000"#));
    }

    #[test]
    fn remote_mode_with_bad_address_falls_back_to_service() {
        for bad in ["", "host:not-a-port", "host:70000", "host:0"] {
            let raw = format!(r#"{{"mode":"remote","remote_rpc_address":"{bad}"}}"#);
            let (split, remotes) = remote_of(&raw);
            assert_eq!(split.runtime, "service");
            assert_eq!(split.target, None);
            assert_eq!(remotes, "[]");
        }
    }

    #[test]
    fn ipv6_addresses_follow_target_ts() {
        let (host, port) = parse_address("[fd00::1]:2000").unwrap();
        assert_eq!(host, "fd00::1");
        assert_eq!(port, 2000);
        // 不带端口的裸 v6 走默认端口
        let (host, port) = parse_address("[fd00::1]").unwrap();
        assert_eq!(host, "fd00::1");
        assert_eq!(port, crate::DEFAULT_RPC_PORT);
        // 未加方括号的 v6 按最后一个 `:` 切端口（与 JS 的 split 行为一致）
        let (host, port) = parse_address(&format!("fd00::1:{}", crate::DEFAULT_RPC_PORT)).unwrap();
        assert_eq!(host, "fd00::1");
        assert_eq!(port, crate::DEFAULT_RPC_PORT);
    }

    #[test]
    fn invalid_app_mode_is_reported_not_guessed() {
        assert!(matches!(split("not json"), AppModeOutcome::Invalid(_)));
        assert!(matches!(
            split(r#"{"mode":"weird"}"#),
            AppModeOutcome::Invalid(_)
        ));
        assert!(matches!(split(r#"["array"]"#), AppModeOutcome::Invalid(_)));
        // 解析失败时不能声称「已经消费掉旧键」。
        assert!(!has_consumed_keys("not json"));
    }
}
