//! 服务 IPC 协议（`proto = 2`）。
//!
//! 这是**唯一真源**：服务端（`exvlan-service`）与所有客户端（GUI / CLI / gateway）
//! 都依赖这里的类型，字段写错会变成编译错误 —— 以前两边手写 JSON 字面量，
//! 改一个漏一个不会有任何提示。
//!
//! wire 格式（v2，带 `params` 嵌套）：
//!
//! ```json
//! {"proto": 2, "op": "config.write", "params": {"id": "…", "text": "…", "enabled": true}}
//! {"proto": 2, "op": "ping"}
//! ```
//!
//! 与 v1 的差别：字段从平铺改成 `params` 对象。这是**破坏性变更**，因此
//! [`PROTO`] 从 1 提到 2，服务端会拒绝低版本客户端并给出明确错误。
//!
//! 响应仍是统一信封（[`Response`]），`data` 由各 op 自行约定（暂时用 JSON）。

use serde::{Deserialize, Serialize};

/// 当前协议版本。服务端只接受等于它的请求。
pub const PROTO: u32 = 2;

/// 网关托管配置（`gateway.set` 的增量修改）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GatewayPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

/// 请求体（不带 `proto`，由 [`WireRequest`] 包一层）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", content = "params")]
pub enum Request {
    #[serde(rename = "hello")]
    Hello,
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "status")]
    Status,

    #[serde(rename = "mode.get")]
    ModeGet,
    #[serde(rename = "mode.set")]
    ModeSet {
        /// `service|session|stopped`（兼容 `normal` / `off`）
        mode: String,
    },

    #[serde(rename = "kernel.ensure")]
    KernelEnsure,
    #[serde(rename = "kernel.stop")]
    KernelStop,
    #[serde(rename = "kernel.restart")]
    KernelRestart,
    /// 用 staging 目录里的内核包（`kernel` + `kernel.sig` + `version`）替换内核。
    #[serde(rename = "apply")]
    Apply { staging: String },

    #[serde(rename = "service.update")]
    ServiceUpdate {
        staging: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
    },

    #[serde(rename = "whitelist.get")]
    WhitelistGet,
    #[serde(rename = "whitelist.set")]
    WhitelistSet {
        #[serde(default)]
        extra: Vec<String>,
    },

    #[serde(rename = "config.list")]
    ConfigList,
    #[serde(rename = "config.read")]
    ConfigRead { id: String },
    #[serde(rename = "config.write")]
    ConfigWrite {
        id: String,
        text: String,
        #[serde(default = "default_true")]
        enabled: bool,
    },
    #[serde(rename = "config.delete")]
    ConfigDelete { id: String },

    #[serde(rename = "port.owner")]
    PortOwner,
    #[serde(rename = "port.cleanup")]
    PortCleanup,

    #[serde(rename = "gateway.status")]
    GatewayStatus,
    #[serde(rename = "gateway.set")]
    GatewaySet { gateway: GatewayPatch },
    #[serde(rename = "gateway.restart")]
    GatewayRestart,
    /// 生成本机一次性配对码（被管端动作）。
    #[serde(rename = "gateway.pair.start")]
    GatewayPairStart {
        #[serde(default = "default_role")]
        role: String,
        #[serde(default = "default_ttl")]
        ttl: u64,
    },

    /// 启动内核升级任务：服务自己下载 + 校验 + 验签 + 安装。
    #[serde(rename = "kernel.install.start")]
    KernelInstallStart {
        version: String,
        /// `stable|rc|beta|dev|nightly`
        channel: String,
        /// 覆盖镜像根（缺省走 service.toml / 编译期常量）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        base: Option<String>,
    },
    /// 查询升级任务进度。
    #[serde(rename = "kernel.install.status")]
    KernelInstallStatus { id: String },
}

fn default_true() -> bool {
    true
}

fn default_role() -> String {
    "readonly".to_string()
}

fn default_ttl() -> u64 {
    300
}

/// 最外层请求：`{proto, op, params}`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireRequest {
    pub proto: u32,
    #[serde(flatten)]
    pub request: Request,
}

impl WireRequest {
    pub fn new(request: Request) -> Self {
        Self {
            proto: PROTO,
            request,
        }
    }
}

/// 统一响应信封。
///
/// 注意：`data` 仍是 JSON（各 op 的负载结构还没逐个类型化）。等把
/// `Response` 的负载也收成枚举后，这里会换成 `Option<Payload>`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proto: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    /// `service|session|stopped`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel: Option<KernelInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gateway: Option<GatewayInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelInfo {
    pub pid: u32,
    pub running: bool,
    pub rpc: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayInfo {
    pub pid: u32,
    pub running: bool,
    pub addr: String,
    pub enabled: bool,
}

impl Response {
    pub fn ok() -> Self {
        Self {
            ok: true,
            proto: None,
            service: None,
            mode: None,
            kernel: None,
            gateway: None,
            code: None,
            message: None,
            data: None,
        }
    }

    pub fn err(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            proto: None,
            service: None,
            mode: None,
            kernel: None,
            gateway: None,
            code: Some(code.into()),
            message: Some(message.into()),
            data: None,
        }
    }
}

/// 内核升级任务状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallState {
    Queued,
    Downloading,
    Verifying,
    Installing,
    Restarting,
    Done,
    Failed,
}

impl InstallState {
    pub fn as_str(self) -> &'static str {
        match self {
            InstallState::Queued => "queued",
            InstallState::Downloading => "downloading",
            InstallState::Verifying => "verifying",
            InstallState::Installing => "installing",
            InstallState::Restarting => "restarting",
            InstallState::Done => "done",
            InstallState::Failed => "failed",
        }
    }
}

/// `kernel.install.status` 的负载。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallStatus {
    pub job_id: String,
    pub state: InstallState,
    /// 0–100；未知时 -1
    pub progress: i32,
    pub version: String,
    #[serde(default)]
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(default)]
    pub rolled_back: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_variant_has_no_params() {
        let wire = WireRequest::new(Request::Ping);
        assert_eq!(
            serde_json::to_string(&wire).unwrap(),
            r#"{"proto":2,"op":"ping"}"#
        );
    }

    #[test]
    fn struct_variant_nests_params() {
        let wire = WireRequest::new(Request::ConfigWrite {
            id: "n1".into(),
            text: "ipv4 = \"10.0.0.1/24\"".into(),
            enabled: true,
        });
        let json = serde_json::to_value(&wire).unwrap();
        assert_eq!(json["op"], "config.write");
        assert_eq!(json["proto"], 2);
        assert_eq!(json["params"]["id"], "n1");
        assert_eq!(json["params"]["enabled"], true);
    }

    #[test]
    fn dotted_op_names_roundtrip() {
        for op in [
            Request::ModeGet,
            Request::KernelRestart,
            Request::ConfigList,
            Request::PortOwner,
            Request::GatewayStatus,
        ] {
            let text = serde_json::to_string(&WireRequest::new(op)).unwrap();
            let back: WireRequest = serde_json::from_str(&text).unwrap();
            assert_eq!(
                serde_json::to_string(&WireRequest::new(back.request)).unwrap(),
                text
            );
        }
    }

    #[test]
    fn install_start_defaults() {
        let wire: WireRequest =
            serde_json::from_str(r#"{"proto":2,"op":"kernel.install.start","params":{"version":"1","channel":"dev"}}"#)
                .unwrap();
        match wire.request {
            Request::KernelInstallStart { base, channel, .. } => {
                assert!(base.is_none());
                assert_eq!(channel, "dev");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn config_write_defaults_enabled_to_true() {
        let wire: WireRequest = serde_json::from_str(
            r#"{"proto":2,"op":"config.write","params":{"id":"n1","text":"x"}}"#,
        )
        .unwrap();
        match wire.request {
            Request::ConfigWrite { enabled, .. } => assert!(enabled),
            other => panic!("unexpected: {other:?}"),
        }
    }
}
