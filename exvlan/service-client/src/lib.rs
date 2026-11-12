//! EXVlan 本机控制面客户端的基础层。
//!
//! 只解决「怎么连、对面是谁」，**不含任何策略**：
//!
//! - [`ipc`]：特权服务 IPC（unix socket / Windows 命名管道），`{proto:1,op}` 行式 JSON；
//! - [`backend`]：探测当前是 `core`（裸核，无服务）还是 `service`（有特权服务）；
//! - [`config`]：网络配置（内核 `config-dir/*.toml`）唯一写者入口；
//! - [`paths`]：三平台统一的目录/二进制/凭据位置。
//!
//! 使用方：GUI（`packages/studio/src-tauri`）、`exvlan-cli`、`exvlan-gateway`。
//! 编排逻辑（装服务 / 起停内核 / 策略 / 升级回滚）**不在这里** —— 那是
//! `exvlan-service` 的职责。

#![allow(clippy::result_large_err)]

pub mod backend;
pub mod config;
pub mod doctor;
pub mod identity;
pub mod ipc;
pub mod paths;
pub mod tokens;

pub use backend::{Backend, detect};
pub use identity::{current_uid, current_user_sid};
pub use ipc::{Client, request};
pub use paths::{APP_ID, PRIVILEGED_SERVICE_LABEL, Paths, paths};

/// 内核 RPC portal 默认端口（`service.toml [kernel] rpc_port` 可覆盖）。
pub const DEFAULT_RPC_PORT: u16 = 1995;
/// gateway 默认端口（`service.toml [gateway] port` 可覆盖）。
pub const DEFAULT_GATEWAY_PORT: u16 = 2016;
/// 本地 RPC 地址（回环）。
pub fn local_rpc_addr() -> String {
    format!("127.0.0.1:{DEFAULT_RPC_PORT}")
}
/// 本地 gateway 地址（回环）。
pub fn local_gateway_addr() -> String {
    format!("127.0.0.1:{DEFAULT_GATEWAY_PORT}")
}
