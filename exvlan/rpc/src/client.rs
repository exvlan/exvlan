//! RpcClient 模块
//!
//! 提供与 RPC 服务器通信的客户端实现。

use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::sync::Mutex;

use exvlan::common::PeerId;
use exvlan::proto::rpc_impl::client::Client;

use crate::error::RpcError;
use crate::services::*;

/// RPC 客户端，用于与 era-xvlan 的 RPC 服务进行通信
///
/// # 示例
/// ```ignore
/// use exvlan_rpc::client::RpcClient;
///
/// #[tokio::main]
/// async fn main() -> Result<(), exvlan_rpc::error::RpcError> {
///     let client = RpcClient::connect("127.0.0.1:50051").await?;
///     // 使用客户端...
///     Ok(())
/// }
/// ```
pub struct RpcClient {
    client: Arc<Mutex<Client>>,
    from_peer_id: PeerId,
    to_peer_id: PeerId,
    domain_name: String,
}

impl RpcClient {
    pub fn new(
        client: Arc<Mutex<Client>>,
        from_peer_id: PeerId,
        to_peer_id: PeerId,
        domain_name: String,
    ) -> Self {
        Self {
            client,
            from_peer_id,
            to_peer_id,
            domain_name,
        }
    }

    /// 连接到 RPC 服务器
    ///
    /// # 参数
    /// - `addr` - RPC 服务器的地址，格式为 `host:port`
    ///
    /// # 返回值
    /// - 成功时返回 `RpcClient` 实例
    /// - 失败时返回 `RpcError`
    pub async fn connect(addr: &str) -> Result<Self, RpcError> {
        let addr: SocketAddr = addr.parse()?;
        let _stream = TcpStream::connect(addr).await?;
        let client = Client::new();

        let from_peer_id = 0;
        let to_peer_id = 0;
        let domain_name = "default".to_string();

        Ok(Self {
            client: Arc::new(Mutex::new(client)),
            from_peer_id,
            to_peer_id,
            domain_name,
        })
    }

    pub fn from_peer_id(&self) -> PeerId {
        self.from_peer_id
    }

    pub fn to_peer_id(&self) -> PeerId {
        self.to_peer_id
    }

    pub fn domain_name(&self) -> &str {
        &self.domain_name
    }

    pub fn client(&self) -> Arc<Mutex<Client>> {
        self.client.clone()
    }

    /// 获取对等节点管理客户端
    ///
    /// # 返回值
    /// - `PeerManageClient` 实例
    pub fn peer_manage(&self) -> PeerManageClient {
        PeerManageClient::new(
            self.client.clone(),
            self.from_peer_id,
            self.to_peer_id,
            self.domain_name.clone(),
        )
    }

    /// 获取连接器管理客户端
    ///
    /// # 返回值
    /// - `ConnectorManageClient` 实例
    pub fn connector_manage(&self) -> ConnectorManageClient {
        ConnectorManageClient::new(
            self.client.clone(),
            self.from_peer_id,
            self.to_peer_id,
            self.domain_name.clone(),
        )
    }

    /// 获取映射监听器管理客户端
    ///
    /// # 返回值
    /// - `MappedListenerManageClient` 实例
    pub fn mapped_listener_manage(&self) -> MappedListenerManageClient {
        MappedListenerManageClient::new(
            self.client.clone(),
            self.from_peer_id,
            self.to_peer_id,
            self.domain_name.clone(),
        )
    }

    /// 获取 VPN 门户客户端
    ///
    /// # 返回值
    /// - `VpnPortalClient` 实例
    pub fn vpn_portal(&self) -> VpnPortalClient {
        VpnPortalClient::new(
            self.client.clone(),
            self.from_peer_id,
            self.to_peer_id,
            self.domain_name.clone(),
        )
    }

    /// 获取 TCP 代理客户端
    ///
    /// # 参数
    /// - `client_type` - 客户端类型
    ///
    /// # 返回值
    /// - `TcpProxyClient` 实例
    pub fn tcp_proxy(&self, client_type: &str) -> TcpProxyClient {
        TcpProxyClient::new(
            self.client.clone(),
            self.from_peer_id,
            self.to_peer_id,
            self.domain_name.clone(),
            client_type.to_string(),
        )
    }

    /// 获取 ACL 管理客户端
    ///
    /// # 返回值
    /// - `AclManageClient` 实例
    pub fn acl_manage(&self) -> AclManageClient {
        AclManageClient::new(
            self.client.clone(),
            self.from_peer_id,
            self.to_peer_id,
            self.domain_name.clone(),
        )
    }

    /// 获取端口转发管理客户端
    ///
    /// # 返回值
    /// - `PortForwardManageClient` 实例
    pub fn port_forward_manage(&self) -> PortForwardManageClient {
        PortForwardManageClient::new(
            self.client.clone(),
            self.from_peer_id,
            self.to_peer_id,
            self.domain_name.clone(),
        )
    }

    /// 获取统计信息客户端
    ///
    /// # 返回值
    /// - `StatsClient` 实例
    pub fn stats(&self) -> StatsClient {
        StatsClient::new(
            self.client.clone(),
            self.from_peer_id,
            self.to_peer_id,
            self.domain_name.clone(),
        )
    }

    /// 获取配置管理客户端
    ///
    /// # 返回值
    /// - `ConfigClient` 实例
    pub fn config(&self) -> ConfigClient {
        ConfigClient::new(
            self.client.clone(),
            self.from_peer_id,
            self.to_peer_id,
            self.domain_name.clone(),
        )
    }

    /// 获取日志管理客户端
    ///
    /// # 返回值
    /// - `LoggerClient` 实例
    pub fn logger(&self) -> LoggerClient {
        LoggerClient::new(
            self.client.clone(),
            self.from_peer_id,
            self.to_peer_id,
            self.domain_name.clone(),
        )
    }

    /// 获取对等节点中心客户端
    ///
    /// # 返回值
    /// - `PeerCenterClient` 实例
    pub fn peer_center(&self) -> PeerCenterClient {
        PeerCenterClient::new(
            self.client.clone(),
            self.from_peer_id,
            self.to_peer_id,
            self.domain_name.clone(),
        )
    }

    /// 获取凭证管理客户端
    ///
    /// # 返回值
    /// - `CredentialManageClient` 实例
    pub fn credential_manage(&self) -> CredentialManageClient {
        CredentialManageClient::new(
            self.client.clone(),
            self.from_peer_id,
            self.to_peer_id,
            self.domain_name.clone(),
        )
    }

    /// 获取实例管理客户端
    ///
    /// # 返回值
    /// - `InstanceManageClient` 实例
    pub fn instance_manage(&self) -> InstanceManageClient {
        InstanceManageClient::new(
            self.client.clone(),
            self.from_peer_id,
            self.to_peer_id,
            self.domain_name.clone(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rpc_client_connect_refused() {
        let result = RpcClient::connect("127.0.0.1:50051").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_rpc_client_connect_invalid_address() {
        let result = RpcClient::connect("invalid-address").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_rpc_client_connect_invalid_ip() {
        let result = RpcClient::connect("999.999.999.999:50051").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_rpc_client_connect_invalid_port() {
        let result = RpcClient::connect("127.0.0.1:99999").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_rpc_client_connect_localhost() {
        let result = RpcClient::connect("localhost:50051").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_rpc_client_connect_ipv6() {
        let result = RpcClient::connect("[::1]:50051").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_rpc_client_new() {
        let client = Arc::new(Mutex::new(Client::new()));
        let from_peer_id = 42;
        let to_peer_id = 100;
        let domain_name = "test.example.com".to_string();

        let rpc_client = RpcClient::new(
            client.clone(),
            from_peer_id,
            to_peer_id,
            domain_name.clone(),
        );

        assert_eq!(rpc_client.from_peer_id(), from_peer_id);
        assert_eq!(rpc_client.to_peer_id(), to_peer_id);
        assert_eq!(rpc_client.domain_name(), domain_name.as_str());
    }

    #[tokio::test]
    async fn test_rpc_client_new_zero_peers() {
        let client = Arc::new(Mutex::new(Client::new()));
        let rpc_client = RpcClient::new(client, 0, 0, "zero.test".to_string());

        assert_eq!(rpc_client.from_peer_id(), 0);
        assert_eq!(rpc_client.to_peer_id(), 0);
        assert_eq!(rpc_client.domain_name(), "zero.test");
    }

    #[tokio::test]
    async fn test_rpc_client_new_max_peers() {
        let client = Arc::new(Mutex::new(Client::new()));
        let max_u32 = u32::MAX;
        let rpc_client = RpcClient::new(client, max_u32, max_u32, "max.test".to_string());

        assert_eq!(rpc_client.from_peer_id(), max_u32);
        assert_eq!(rpc_client.to_peer_id(), max_u32);
    }

    #[tokio::test]
    async fn test_rpc_client_new_empty_domain() {
        let client = Arc::new(Mutex::new(Client::new()));
        let rpc_client = RpcClient::new(client, 0, 0, String::new());

        assert_eq!(rpc_client.domain_name(), "");
    }

    #[tokio::test]
    async fn test_rpc_client_new_special_chars_domain() {
        let client = Arc::new(Mutex::new(Client::new()));
        let domain = "test-domain_123.example.com".to_string();
        let rpc_client = RpcClient::new(client, 0, 0, domain.clone());

        assert_eq!(rpc_client.domain_name(), domain);
    }

    #[tokio::test]
    async fn test_rpc_client_services_all() {
        let client = Arc::new(Mutex::new(Client::new()));
        let rpc_client = RpcClient::new(client, 10, 20, "services.test".to_string());

        let peer_manage = rpc_client.peer_manage();
        assert_eq!(peer_manage.from_peer_id, 10);
        assert_eq!(peer_manage.to_peer_id, 20);
        assert_eq!(peer_manage.domain_name, "services.test");

        let connector_manage = rpc_client.connector_manage();
        assert_eq!(connector_manage.from_peer_id, 10);
        assert_eq!(connector_manage.to_peer_id, 20);

        let mapped_listener = rpc_client.mapped_listener_manage();
        assert_eq!(mapped_listener.from_peer_id, 10);
        assert_eq!(mapped_listener.domain_name, "services.test");

        let vpn_portal = rpc_client.vpn_portal();
        assert_eq!(vpn_portal.to_peer_id, 20);

        let tcp_proxy = rpc_client.tcp_proxy("proxy_type");
        assert_eq!(tcp_proxy.from_peer_id, 10);
        assert_eq!(tcp_proxy.to_peer_id, 20);
        assert_eq!(tcp_proxy.client_type(), "proxy_type");

        let acl_manage = rpc_client.acl_manage();
        assert_eq!(acl_manage.domain_name, "services.test");

        let port_forward = rpc_client.port_forward_manage();
        assert_eq!(port_forward.from_peer_id, 10);

        let stats = rpc_client.stats();
        assert_eq!(stats.to_peer_id, 20);

        let config = rpc_client.config();
        assert_eq!(config.domain_name, "services.test");

        let logger = rpc_client.logger();
        assert_eq!(logger.from_peer_id, 10);

        let peer_center = rpc_client.peer_center();
        assert_eq!(peer_center.to_peer_id, 20);

        let credential = rpc_client.credential_manage();
        assert_eq!(credential.domain_name, "services.test");

        let instance = rpc_client.instance_manage();
        assert_eq!(instance.from_peer_id, 10);
    }

    #[tokio::test]
    async fn test_rpc_client_services_client_type() {
        let client = Arc::new(Mutex::new(Client::new()));
        let rpc_client = RpcClient::new(client, 0, 0, "test".to_string());

        let tcp_proxy1 = rpc_client.tcp_proxy("type_a");
        assert_eq!(tcp_proxy1.client_type(), "type_a");

        let tcp_proxy2 = rpc_client.tcp_proxy("type_b");
        assert_eq!(tcp_proxy2.client_type(), "type_b");

        let tcp_proxy3 = rpc_client.tcp_proxy("");
        assert_eq!(tcp_proxy3.client_type(), "");
    }
}

/// RPC 客户端管理器，用于管理 RPC 客户端的生命周期
///
/// # 示例
/// ```ignore
/// use exvlan_rpc::client::RpcClientManager;
///
/// #[tokio::main]
/// async fn main() -> Result<(), exvlan_rpc::error::RpcError> {
///     let mut manager = RpcClientManager::new();
///     let client = manager.get_or_create_rpc_client("127.0.0.1:50051").await?;
///     // 使用客户端...
///     Ok(())
/// }
/// ```
pub struct RpcClientManager {
    rpc_client: Option<Arc<Mutex<RpcClient>>>,
}

impl Default for RpcClientManager {
    fn default() -> Self {
        Self::new()
    }
}

impl RpcClientManager {
    /// 创建一个新的 RPC 客户端管理器
    ///
    /// # 返回值
    /// - `RpcClientManager` 实例
    pub fn new() -> Self {
        Self { rpc_client: None }
    }

    /// 获取或创建 RPC 客户端
    ///
    /// # 参数
    /// - `addr` - RPC 服务器的地址，格式为 `host:port`
    ///
    /// # 返回值
    /// - 成功时返回 `Arc<Mutex<RpcClient>>` 实例
    /// - 失败时返回 `RpcError`
    pub async fn get_or_create_rpc_client(
        &mut self,
        addr: &str,
    ) -> Result<Arc<Mutex<RpcClient>>, RpcError> {
        if let Some(rpc_client) = &self.rpc_client {
            return Ok(rpc_client.clone());
        }

        let rpc_client = RpcClient::connect(addr).await?;
        let rpc_client = Arc::new(Mutex::new(rpc_client));
        self.rpc_client = Some(rpc_client.clone());
        Ok(rpc_client)
    }

    /// 获取已存在的 RPC 客户端
    ///
    /// # 返回值
    /// - 如果存在，返回 `Some<Arc<Mutex<RpcClient>>>`
    /// - 如果不存在，返回 `None`
    pub fn get_rpc_client(&self) -> Option<Arc<Mutex<RpcClient>>> {
        self.rpc_client.clone()
    }

    /// 关闭 RPC 客户端
    pub fn close_rpc_client(&mut self) {
        self.rpc_client = None;
    }
}

#[cfg(test)]
mod manager_tests {
    use super::*;

    #[tokio::test]
    async fn test_rpc_client_manager_new() {
        let manager = RpcClientManager::new();
        assert!(manager.get_rpc_client().is_none());
    }

    #[tokio::test]
    async fn test_rpc_client_manager_default() {
        let manager: RpcClientManager = Default::default();
        assert!(manager.get_rpc_client().is_none());
    }

    #[tokio::test]
    async fn test_rpc_client_manager_get_or_create_refused() {
        let mut manager = RpcClientManager::new();
        let result = manager.get_or_create_rpc_client("127.0.0.1:50051").await;
        assert!(result.is_err());
        assert!(manager.get_rpc_client().is_none());
    }

    #[tokio::test]
    async fn test_rpc_client_manager_get_or_create_invalid_addr() {
        let mut manager = RpcClientManager::new();
        let result = manager.get_or_create_rpc_client("invalid").await;
        assert!(result.is_err());
        assert!(manager.get_rpc_client().is_none());
    }

    #[tokio::test]
    async fn test_rpc_client_manager_get_or_create_once() {
        let mut manager = RpcClientManager::new();
        let result1 = manager.get_or_create_rpc_client("127.0.0.1:50051").await;
        let result2 = manager.get_or_create_rpc_client("127.0.0.1:50052").await;

        assert!(result1.is_err());
        assert!(result2.is_err());
    }

    #[tokio::test]
    async fn test_rpc_client_manager_close() {
        let mut manager = RpcClientManager::new();
        let _ = manager.get_or_create_rpc_client("127.0.0.1:50051").await;
        manager.close_rpc_client();
        assert!(manager.get_rpc_client().is_none());
    }

    #[tokio::test]
    async fn test_rpc_client_manager_close_twice() {
        let mut manager = RpcClientManager::new();
        manager.close_rpc_client();
        manager.close_rpc_client();
        assert!(manager.get_rpc_client().is_none());
    }

    #[tokio::test]
    async fn test_rpc_client_manager_get_rpc_client() {
        let manager = RpcClientManager::new();
        let _ = manager.get_rpc_client();
    }

    #[tokio::test]
    async fn test_rpc_client_manager_multiple_instances() {
        let manager1 = RpcClientManager::new();
        let manager2 = RpcClientManager::new();

        assert!(manager1.get_rpc_client().is_none());
        assert!(manager2.get_rpc_client().is_none());
        assert!(!std::ptr::eq(&manager1 as *const _, &manager2 as *const _));
    }
}
