//! exvlan-rpc
//!
//! RPC 客户端库，用于与 era-xvlan 项目的 RPC 服务进行通信。
//!
//! # 功能特性
//! - 支持所有 era-xvlan 的 RPC 服务
//! - 异步 API 设计，基于 Tokio
//! - 统一的错误处理
//! - 模块化的服务客户端设计
//! - 支持多种服务类型
//!
//! # 快速开始
//!
//! ```rust,ignore
//! use exvlan::common::PeerId;
//! use exvlan_rpc::client::RpcClient;
//!
//! #[tokio::main]
//! async fn main() -> Result<(), exvlan_rpc::error::RpcError> {
//!     // 创建 RPC 客户端
//!     let client = RpcClient::connect("127.0.0.1:50051").await?;
//!
//!     // 获取对等节点管理客户端
//!     let peer_manage = client.peer_manage();
//!
//!     // 调用方法
//!     // let peers = peer_manage.list_peer().await?;
//!     // println!("Peers: {:?}", peers);
//!
//!     Ok(())
//! }
//! ```
//!
//! # 导出类型
//!
//! 本库导出以下类型供外部使用：
//!
//! ## Proto 类型 - manage
//!
//! - `WebClientService` - Web 客户端服务 trait
//! - `WebClientServiceClientFactory` - Web 客户端服务工厂
//! - `ValidateConfigRequest` / `ValidateConfigResponse` - 验证配置请求/响应
//! - `RunNetworkInstanceRequest` / `RunNetworkInstanceResponse` - 运行网络实例请求/响应
//! - `RetainNetworkInstanceRequest` / `RetainNetworkInstanceResponse` - 保留网络实例请求/响应
//! - `CollectNetworkInfoRequest` / `CollectNetworkInfoResponse` - 收集网络信息请求/响应
//! - `ListNetworkInstanceRequest` / `ListNetworkInstanceResponse` - 列出网络实例请求/响应
//! - `DeleteNetworkInstanceRequest` / `DeleteNetworkInstanceResponse` - 删除网络实例请求/响应
//! - `GetNetworkInstanceConfigRequest` / `GetNetworkInstanceConfigResponse` - 获取网络实例配置请求/响应
//! - `ListNetworkInstanceMetaRequest` / `ListNetworkInstanceMetaResponse` - 列出网络实例元数据请求/响应
//!
//! ## Proto 类型 - logger
//!
//! - `LoggerRpc` - 日志 RPC trait
//! - `LoggerRpcClientFactory` - 日志 RPC 客户端工厂
//! - `GetLoggerConfigRequest` / `GetLoggerConfigResponse` - 获取日志配置请求/响应
//! - `SetLoggerConfigRequest` / `SetLoggerConfigResponse` - 设置日志配置请求/响应
//!
//! ## Core 类型
//!
//! - `VERSION` - 版本号
//! - `NetworkConfig` - 网络配置
//! - `GlobalCtx` - 全局上下文
//! - `GlobalCtxEvent` - 全局上下文事件
//!
//! ## RPC Service 类型
//!
//! - `RemoteClientManager` - 远程客户端管理器 trait
//! - `Storage` - 存储 trait
//! - `PersistentConfig` - 持久化配置 trait
//! - `RpcClientManager` - RPC 客户端管理器

pub mod client;
pub mod error;
pub mod services;

#[cfg(test)]
pub mod test_utils;

pub use client::{RpcClient, RpcClientManager};

// Proto 类型 - manage
pub use exvlan::proto::api::manage::{
    CollectNetworkInfoRequest, CollectNetworkInfoResponse, DeleteNetworkInstanceRequest,
    DeleteNetworkInstanceResponse, GetNetworkInstanceConfigRequest,
    GetNetworkInstanceConfigResponse, ListNetworkInstanceMetaRequest,
    ListNetworkInstanceMetaResponse, ListNetworkInstanceRequest, ListNetworkInstanceResponse,
    NetworkMeta, RetainNetworkInstanceRequest, RetainNetworkInstanceResponse,
    RunNetworkInstanceRequest, RunNetworkInstanceResponse, ValidateConfigRequest,
    ValidateConfigResponse, WebClientService, WebClientServiceClientFactory,
};

// Proto 类型 - logger
pub use exvlan::proto::api::logger::{
    GetLoggerConfigRequest, GetLoggerConfigResponse, LoggerRpc, LoggerRpcClientFactory,
    SetLoggerConfigRequest, SetLoggerConfigResponse,
};

// Proto 类型 - common & rpc
pub use exvlan::proto::common::NatType;
pub use exvlan::proto::rpc_impl::bidirect::BidirectRpcManager;
pub use exvlan::proto::rpc_types::controller::BaseController;

// Core 类型
pub use exvlan::VERSION;
pub use exvlan::common::config::{
    ConfigLoader, FileLoggerConfig, LoggingConfigBuilder, TomlConfigLoader,
};
pub use exvlan::common::global_ctx::GlobalCtx;
pub use exvlan::common::global_ctx::GlobalCtxEvent;
pub use exvlan::common::stun::MockStunInfoCollector;
pub use exvlan::connector::create_connector_by_url;
pub use exvlan::launcher::NetworkConfig;
pub use exvlan::service_manager::{Service, ServiceInstallOptions, ServiceStatus};
pub use exvlan::tunnel::ring::RingTunnelConnector;
pub use exvlan::tunnel::{IpVersion, TunnelConnector};
pub use exvlan::web_client::WebClientHooks;
pub use exvlan::web_client::{self, WebClient};

// RPC Service 类型
pub use exvlan::rpc_service::logger::LoggerRpcService;
pub use exvlan::rpc_service::remote_client::{
    ListNetworkProps, PersistentConfig, RemoteClientError, RemoteClientManager, Storage,
};

// 错误类型
pub use error::RpcError as RpcClientError;
pub use exvlan::proto::rpc_types::error::Error as RpcError;
