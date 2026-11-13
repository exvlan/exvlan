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
//! 响应是统一信封（[`Response`]）：所有负载都进 `data`，且**带类型**
//! （[`Payload`]）—— 以前两边手写 JSON 字面量，字段对不上不会有任何提示。

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

/// 内核生命周期策略（与 `service.toml` 的 `policy` 同一套值）。
///
/// **规范值是 `resident`**（`service` 是历史别名，服务端以 `resident` 序列化）。
/// 这是冒烟脚本抓出来的真实漂移：协议里曾写成 `service`，与服务端不一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    #[serde(rename = "resident", alias = "service", alias = "Service")]
    Resident,
    #[serde(rename = "session", alias = "normal", alias = "Normal")]
    Session,
    #[serde(rename = "stopped", alias = "off", alias = "Off")]
    Stopped,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Resident => "resident",
            Mode::Session => "session",
            Mode::Stopped => "stopped",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "resident" | "service" => Some(Mode::Resident),
            "session" | "normal" => Some(Mode::Session),
            "stopped" | "off" => Some(Mode::Stopped),
            _ => None,
        }
    }
}

/// 结构化错误码。未知值落到 `Unknown`（前后端版本不同时不至于直接反序列化失败）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    BadRequest,
    NotFound,
    Internal,
    PortInUse,
    KernelStartFailed,
    KernelRestartFailed,
    KernelBusy,
    /// 服务与客户端 IPC 协议版本不一致
    ProtoMismatch,
    UpdateFailed,
    SaveFailed,
    WriteFailed,
    DeleteFailed,
    ConfigWrite,
    ConfigRead,
    BadCidr,
    TooMany,
    Nothing,
    KillFailed,
    /// 端口占用者不是本产品的内核，拒绝终止（`port.cleanup`）
    NotOurs,
    BadPort,
    BadBind,
    GatewayFailed,
    PairFailed,
    /// 切换生命周期策略失败（写 `service.toml` / 起停内核）
    ModeFailed,
    /// `apply` 换内核失败（非回滚路径）
    ApplyFailed,
    /// 新内核起不来，已回滚到上一版
    KernelRollback,
    /// 该 op 已定义但尚未实现（例如 `kernel.install.*`）
    NotImplemented,
    /// 下载失败（网络/404/超时）
    DownloadFailed,
    /// 包内容与清单 sha256 不一致
    Sha256Mismatch,
    /// `kernel.sig` 缺失或验签失败
    SigInvalid,
    /// 安装任务实例不存在
    JobNotFound,
    /// 跨 Mmpp 降级被拒绝
    DowngradeRefused,
    #[serde(other)]
    Unknown,
}

impl ErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorCode::BadRequest => "bad_request",
            ErrorCode::NotFound => "not_found",
            ErrorCode::Internal => "internal",
            ErrorCode::PortInUse => "port_in_use",
            ErrorCode::KernelStartFailed => "kernel_start_failed",
            ErrorCode::KernelRestartFailed => "kernel_restart_failed",
            ErrorCode::KernelBusy => "kernel_busy",
            ErrorCode::ProtoMismatch => "proto_mismatch",
            ErrorCode::UpdateFailed => "update_failed",
            ErrorCode::SaveFailed => "save_failed",
            ErrorCode::WriteFailed => "write_failed",
            ErrorCode::DeleteFailed => "delete_failed",
            ErrorCode::ConfigWrite => "config_write",
            ErrorCode::ConfigRead => "config_read",
            ErrorCode::BadCidr => "bad_cidr",
            ErrorCode::TooMany => "too_many",
            ErrorCode::Nothing => "nothing",
            ErrorCode::KillFailed => "kill_failed",
            ErrorCode::NotOurs => "not_ours",
            ErrorCode::BadPort => "bad_port",
            ErrorCode::BadBind => "bad_bind",
            ErrorCode::GatewayFailed => "gateway_failed",
            ErrorCode::PairFailed => "pair_failed",
            ErrorCode::ModeFailed => "mode_failed",
            ErrorCode::ApplyFailed => "apply_failed",
            ErrorCode::KernelRollback => "kernel_rollback",
            ErrorCode::NotImplemented => "not_implemented",
            ErrorCode::DownloadFailed => "download_failed",
            ErrorCode::Sha256Mismatch => "sha256_mismatch",
            ErrorCode::SigInvalid => "sig_invalid",
            ErrorCode::JobNotFound => "job_not_found",
            ErrorCode::DowngradeRefused => "downgrade_refused",
            ErrorCode::Unknown => "unknown",
        }
    }
}

/// 响应负载。`kind` **与请求的 op 同名**，便于日志里一一对照。
///
/// 字段必须与服务端实际输出逐字段一致：客户端按这些结构反序列化，
/// 改名/删字段就是线上「解析失败」。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Payload {
    #[serde(rename = "hello")]
    Hello { proto: u32, service: String },
    #[serde(rename = "status")]
    Status {
        mode: Mode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kernel: Option<KernelInfo>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gateway: Option<GatewayInfo>,
    },
    #[serde(rename = "mode.get")]
    ModeGet { mode: Mode },
    #[serde(rename = "mode.set")]
    ModeSet { mode: Mode },
    #[serde(rename = "kernel.ensure")]
    KernelEnsure { kernel: Option<KernelInfo> },
    #[serde(rename = "kernel.stop")]
    KernelStop,
    #[serde(rename = "kernel.restart")]
    KernelRestart { kernel: Option<KernelInfo> },
    #[serde(rename = "apply")]
    Apply,
    #[serde(rename = "service.update")]
    ServiceUpdate,
    #[serde(rename = "whitelist.get")]
    WhitelistGet {
        #[serde(flatten)]
        info: WhitelistInfo,
    },
    #[serde(rename = "whitelist.set")]
    WhitelistSet {
        #[serde(flatten)]
        info: WhitelistInfo,
    },
    #[serde(rename = "config.list")]
    ConfigList {
        dir: String,
        items: Vec<ConfigEntry>,
    },
    #[serde(rename = "config.read")]
    ConfigRead {
        id: String,
        enabled: bool,
        text: String,
    },
    #[serde(rename = "config.write")]
    ConfigWrite {
        id: String,
        enabled: bool,
        bytes: u64,
    },
    #[serde(rename = "config.delete")]
    ConfigDelete,
    #[serde(rename = "port.owner")]
    PortOwner {
        port: u16,
        /// 端口空闲时只回 `{port, free: true}`，其余字段缺省。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pid: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        is_our_kernel: Option<bool>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        free: bool,
    },
    #[serde(rename = "port.cleanup")]
    PortCleanup,
    #[serde(rename = "gateway.status")]
    GatewayStatus { gateway: GatewayInfo },
    #[serde(rename = "gateway.set")]
    GatewaySet { gateway: GatewayInfo },
    #[serde(rename = "gateway.restart")]
    GatewayRestart { gateway: GatewayInfo },
    #[serde(rename = "gateway.pair.start")]
    GatewayPairStart {
        code: String,
        expires_in: u64,
        role: String,
        scopes: Vec<String>,
    },
    #[serde(rename = "kernel.install.start")]
    KernelInstallStart { job_id: String },
    #[serde(rename = "kernel.install.status")]
    KernelInstallStatus { status: InstallStatus },
    /// `port_in_use` 错误的负载（`ok=false` 也带 `data`，GUI 据此渲染冲突卡片）。
    ///
    /// 注：v1 里「占用者性质」的字段名是 `kind`，而 v2 的 `kind` 已经是信封标签
    /// （serde 内部标签与同名字段互斥），故改名为 `owner_kind`，信息不丢。
    #[serde(rename = "port_in_use")]
    PortInUse {
        port: u16,
        occupant: Occupant,
        /// `exvlan` / `forwarded` / `foreign`
        owner_kind: String,
        hint: String,
    },
}

/// 端口占用者。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Occupant {
    pub pid: u32,
    pub name: String,
    /// 可执行文件路径（拿不到权限时为空串）。
    pub path: String,
}

/// `config.list` 的一条配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigEntry {
    pub id: String,
    pub enabled: bool,
    pub bytes: u64,
    /// 最后修改时间（UNIX 秒）；拿不到时为 0。
    pub modified: u64,
    /// 配置正文（列表一次带回来，避免客户端逐条 `config.read`）。
    pub text: String,
}

/// `whitelist.get` / `whitelist.set` 的负载：RPC 白名单的推导结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhitelistInfo {
    /// 生效白名单（逗号分隔）。
    pub effective: String,
    /// 内核启动时实际用的白名单；`None` = 内核还没由本服务拉起过。
    pub applied: Option<String>,
    /// 配置改了但内核还没重启（新网段暂不生效）。
    pub needs_restart: bool,
    pub loopback: Vec<String>,
    pub networks: Vec<String>,
    pub extra: Vec<String>,
    pub notes: Vec<String>,
    pub dhcp_fallback: String,
    pub max_extra: usize,
}

/// 统一响应信封：`{ok, proto?, code?, message?, data?}`。
///
/// 所有负载都在 `data` 里（不再把 mode/kernel/gateway 摊在顶层）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proto: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<ErrorCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Payload>,
}

impl Response {
    pub fn ok(data: Payload) -> Self {
        Self {
            ok: true,
            proto: Some(PROTO),
            code: None,
            message: None,
            data: Some(data),
        }
    }

    /// 无负载的成功响应。
    pub fn ok_empty() -> Self {
        Self {
            ok: true,
            proto: Some(PROTO),
            code: None,
            message: None,
            data: None,
        }
    }

    pub fn err(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            proto: Some(PROTO),
            code: Some(code),
            message: Some(message.into()),
            data: None,
        }
    }

    /// 带结构化负载的错误（例如 `port_in_use` 的 occupant/kind/hint）。
    pub fn err_with_data(code: ErrorCode, message: impl Into<String>, data: Payload) -> Self {
        Self {
            data: Some(data),
            ..Self::err(code, message)
        }
    }
}

/// 内核进程状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelInfo {
    pub pid: u32,
    pub running: bool,
    pub rpc: String,
}

/// 托管的 gateway 状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GatewayInfo {
    pub pid: u32,
    pub running: bool,
    pub addr: String,
    pub enabled: bool,
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
        let wire: WireRequest = serde_json::from_str(
            r#"{"proto":2,"op":"kernel.install.start","params":{"version":"1","channel":"dev"}}"#,
        )
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
    fn response_carries_payload_in_data() {
        let resp = Response::ok(Payload::Status {
            mode: Mode::Resident,
            kernel: Some(KernelInfo {
                pid: 7,
                running: true,
                rpc: crate::local_rpc_addr(),
            }),
            gateway: None,
        });
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["ok"], true);
        assert_eq!(json["proto"], 2);
        assert_eq!(json["data"]["kind"], "status");
        assert_eq!(json["data"]["mode"], "resident");
        assert_eq!(json["data"]["kernel"]["pid"], 7);
        // 旧的信封顶层字段不再出现
        assert!(json.get("mode").is_none());
        assert!(json.get("kernel").is_none());
    }

    #[test]
    fn payload_kind_matches_op_name() {
        let cases = [
            (
                Payload::ConfigList {
                    dir: "config-dir".into(),
                    items: Vec::new(),
                },
                "config.list",
            ),
            (Payload::PortCleanup, "port.cleanup"),
            (Payload::KernelStop, "kernel.stop"),
        ];
        for (payload, want) in cases {
            let json = serde_json::to_value(&payload).unwrap();
            assert_eq!(json["kind"], want);
        }
    }

    #[test]
    fn whitelist_payload_is_flat() {
        let payload = Payload::WhitelistGet {
            info: WhitelistInfo {
                effective: "127.0.0.0/8,::1/128".into(),
                applied: None,
                needs_restart: false,
                loopback: vec!["127.0.0.0/8".into()],
                networks: Vec::new(),
                extra: Vec::new(),
                notes: Vec::new(),
                dhcp_fallback: "10.254.0.0/16".into(),
                max_extra: 8,
            },
        };
        let json = serde_json::to_value(&payload).unwrap();
        // 负载字段必须摊在 `kind` 同级（客户端按这个形状读），不能嵌一层。
        assert_eq!(json["kind"], "whitelist.get");
        assert_eq!(json["effective"], "127.0.0.0/8,::1/128");
        assert_eq!(json["max_extra"], 8);
        assert!(json.get("info").is_none());
        let back: Payload = serde_json::from_value(json).unwrap();
        assert!(matches!(back, Payload::WhitelistGet { .. }));
    }

    #[test]
    fn port_owner_free_omits_occupant_fields() {
        let free = serde_json::to_value(Payload::PortOwner {
            port: 4567,
            pid: None,
            name: None,
            path: None,
            is_our_kernel: None,
            free: true,
        })
        .unwrap();
        assert_eq!(free["kind"], "port.owner");
        assert_eq!(free["free"], true);
        assert!(free.get("pid").is_none());

        let used = serde_json::to_value(Payload::PortOwner {
            port: 4567,
            pid: Some(7),
            name: Some("exvlan".into()),
            path: Some("exvlan-bin".into()),
            is_our_kernel: Some(true),
            free: false,
        })
        .unwrap();
        assert_eq!(used["pid"], 7);
        assert!(used.get("free").is_none());
    }

    #[test]
    fn error_response_can_carry_data() {
        let resp = Response::err_with_data(
            ErrorCode::PortInUse,
            "RPC 端口 4567 被 Code（pid=1, kind=forwarded）占用",
            Payload::PortInUse {
                port: 4567,
                occupant: Occupant {
                    pid: 1,
                    name: "Code".into(),
                    path: "/usr/bin/code".into(),
                },
                owner_kind: "forwarded".into(),
                hint: "停掉转发".into(),
            },
        );
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["ok"], false);
        assert_eq!(json["code"], "port_in_use");
        assert_eq!(json["data"]["kind"], "port_in_use");
        assert_eq!(json["data"]["occupant"]["pid"], 1);
    }

    #[test]
    fn error_code_unknown_is_tolerated() {
        let err: Response =
            serde_json::from_str(r#"{"ok":false,"code":"brand_new_code","message":"x"}"#).unwrap();
        assert_eq!(err.code, Some(ErrorCode::Unknown));
        // 已知值往返
        let known = Response::err(ErrorCode::ProtoMismatch, "期望 2");
        let back: Response = serde_json::from_str(&serde_json::to_string(&known).unwrap()).unwrap();
        assert_eq!(back.code, Some(ErrorCode::ProtoMismatch));
    }

    #[test]
    fn mode_accepts_legacy_aliases() {
        for (raw, want) in [
            ("\"normal\"", Mode::Session),
            ("\"off\"", Mode::Stopped),
            ("\"service\"", Mode::Resident),
            ("\"resident\"", Mode::Resident),
        ] {
            let got: Mode = serde_json::from_str(raw).unwrap();
            assert_eq!(got, want);
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
