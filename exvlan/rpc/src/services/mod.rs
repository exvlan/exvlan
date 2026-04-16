//! 服务客户端模块
//!
//! 提供各种服务的客户端实现。

pub mod acl_manage;
pub mod config;
pub mod connector_manage;
pub mod credential_manage;
pub mod instance_manage;
pub mod logger;
pub mod mapped_listener_manage;
pub mod peer_center;
pub mod peer_manage;
pub mod port_forward_manage;
pub mod proxy;
pub mod stats;
pub mod vpn_portal;

/// ACL 管理客户端
pub use acl_manage::AclManageClient;
/// 配置管理客户端
pub use config::ConfigClient;
/// 连接器管理客户端
pub use connector_manage::ConnectorManageClient;
/// 凭证管理客户端
pub use credential_manage::CredentialManageClient;
/// 实例管理客户端
pub use instance_manage::InstanceManageClient;
/// 日志管理客户端
pub use logger::LoggerClient;
/// 映射监听器管理客户端
pub use mapped_listener_manage::MappedListenerManageClient;
/// 对等节点中心客户端
pub use peer_center::PeerCenterClient;
/// 对等节点管理客户端
pub use peer_manage::PeerManageClient;
/// 端口转发管理客户端
pub use port_forward_manage::PortForwardManageClient;
/// TCP 代理客户端
pub use proxy::TcpProxyClient;
/// 统计信息客户端
pub use stats::StatsClient;
/// VPN 门户客户端
pub use vpn_portal::VpnPortalClient;