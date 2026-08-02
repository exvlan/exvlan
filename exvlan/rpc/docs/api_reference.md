# API 参考

本文档提供了 `exvlan-rpc` 客户端库的详细 API 参考。

## 核心类型

### RpcClient

`RpcClient` 是主要的客户端类型，用于创建和管理与 RPC 服务器的连接。

#### 方法

##### `new(addr: &str, from_peer_id: PeerId, to_peer_id: PeerId, domain_name: String) -> Result<Self, RpcError>`

创建一个新的 RPC 客户端，连接到指定的地址。

- **参数**：
  - `addr` - RPC 服务器的地址，格式为 `host:port`
  - `from_peer_id` - 源对等节点 ID
  - `to_peer_id` - 目标对等节点 ID
  - `domain_name` - 域名
- **返回值**：
  - 成功时返回 `RpcClient` 实例
  - 失败时返回 `RpcError`

##### `peer_manage(&self, from_peer_id: PeerId, to_peer_id: PeerId, domain_name: String) -> PeerManageClient`

获取对等节点管理客户端。

##### `connector_manage(&self, from_peer_id: PeerId, to_peer_id: PeerId, domain_name: String) -> ConnectorManageClient`

获取连接器管理客户端。

##### `mapped_listener_manage(&self, from_peer_id: PeerId, to_peer_id: PeerId, domain_name: String) -> MappedListenerManageClient`

获取映射监听器管理客户端。

##### `vpn_portal(&self, from_peer_id: PeerId, to_peer_id: PeerId, domain_name: String) -> VpnPortalClient`

获取 VPN 门户客户端。

##### `tcp_proxy(&self, from_peer_id: PeerId, to_peer_id: PeerId, domain_name: String, client_type: String) -> TcpProxyClient`

获取 TCP 代理客户端。

##### `acl_manage(&self, from_peer_id: PeerId, to_peer_id: PeerId, domain_name: String) -> AclManageClient`

获取 ACL 管理客户端。

##### `port_forward_manage(&self, from_peer_id: PeerId, to_peer_id: PeerId, domain_name: String) -> PortForwardManageClient`

获取端口转发管理客户端。

##### `stats(&self, from_peer_id: PeerId, to_peer_id: PeerId, domain_name: String) -> StatsClient`

获取统计信息客户端。

##### `config(&self, from_peer_id: PeerId, to_peer_id: PeerId, domain_name: String) -> ConfigClient`

获取配置管理客户端。

##### `logger(&self, from_peer_id: PeerId, to_peer_id: PeerId, domain_name: String) -> LoggerClient`

获取日志管理客户端。

##### `instance_manage(&self, from_peer_id: PeerId, to_peer_id: PeerId, domain_name: String) -> InstanceManageClient`

获取实例管理客户端。

##### `peer_center(&self, from_peer_id: PeerId, to_peer_id: PeerId, domain_name: String) -> PeerCenterClient`

获取对等节点中心客户端。

##### `credential_manage(&self, from_peer_id: PeerId, to_peer_id: PeerId, domain_name: String) -> CredentialManageClient`

获取凭证管理客户端。

## 服务客户端

### 1. PeerManageClient

对等节点管理客户端。

#### 方法

##### `list_peer(&self, request: ListPeerRequest) -> anyhow::Result<ListPeerResponse>`

列出所有对等节点。

##### `list_route(&self, request: ListRouteRequest) -> anyhow::Result<ListRouteResponse>`

列出所有路由。

##### `dump_route(&self, request: DumpRouteRequest) -> anyhow::Result<DumpRouteResponse>`

导出路由。

### 2. ConnectorManageClient

连接器管理客户端。

#### 方法

##### `list_connector(&self, request: ListConnectorRequest) -> anyhow::Result<ListConnectorResponse>`

列出所有连接器。

### 3. MappedListenerManageClient

映射监听器管理客户端。

#### 方法

##### `list_mapped_listener(&self, request: ListMappedListenerRequest) -> anyhow::Result<ListMappedListenerResponse>`

列出所有映射监听器。

### 4. VpnPortalClient

VPN 门户客户端。

#### 方法

##### `get_vpn_portal_info(&self, request: GetVpnPortalInfoRequest) -> anyhow::Result<GetVpnPortalInfoResponse>`

获取 VPN 门户信息。

### 5. TcpProxyClient

TCP 代理客户端。

#### 方法

##### `list_tcp_proxy_entry(&self, request: ListTcpProxyEntryRequest) -> anyhow::Result<ListTcpProxyEntryResponse>`

列出所有 TCP 代理条目。

### 6. AclManageClient

ACL 管理客户端。

#### 方法

##### `get_acl_stats(&self, request: GetAclStatsRequest) -> anyhow::Result<GetAclStatsResponse>`

获取 ACL 统计信息。

##### `get_whitelist(&self, request: GetWhitelistRequest) -> anyhow::Result<GetWhitelistResponse>`

获取白名单。

### 7. PortForwardManageClient

端口转发管理客户端。

#### 方法

##### `list_port_forward(&self, request: ListPortForwardRequest) -> anyhow::Result<ListPortForwardResponse>`

列出所有端口转发规则。

### 8. StatsClient

统计信息客户端。

#### 方法

##### `get_stats(&self, request: GetStatsRequest) -> anyhow::Result<GetStatsResponse>`

获取统计信息。

##### `get_prometheus_stats(&self, request: GetPrometheusStatsRequest) -> anyhow::Result<GetPrometheusStatsResponse>`

获取 Prometheus 格式的统计信息。

### 9. ConfigClient

配置管理客户端。

#### 方法

##### `get_config(&self, request: GetConfigRequest) -> anyhow::Result<GetConfigResponse>`

获取配置。

##### `patch_config(&self, request: PatchConfigRequest) -> anyhow::Result<PatchConfigResponse>`

更新配置。

### 10. LoggerClient

日志管理客户端。

#### 方法

##### `get_logger_config(&self, request: GetLoggerConfigRequest) -> anyhow::Result<GetLoggerConfigResponse>`

获取日志配置。

##### `set_logger_config(&self, request: SetLoggerConfigRequest) -> anyhow::Result<SetLoggerConfigResponse>`

设置日志配置。

### 11. InstanceManageClient

实例管理客户端。

#### 方法

##### `validate_config(&self, request: ValidateConfigRequest) -> anyhow::Result<ValidateConfigResponse>`

验证配置。

##### `run_network_instance(&self, request: RunNetworkInstanceRequest) -> anyhow::Result<RunNetworkInstanceResponse>`

运行网络实例。

##### `retain_network_instance(&self, request: RetainNetworkInstanceRequest) -> anyhow::Result<RetainNetworkInstanceResponse>`

保留网络实例。

##### `collect_network_info(&self, request: CollectNetworkInfoRequest) -> anyhow::Result<CollectNetworkInfoResponse>`

收集网络信息。

##### `list_network_instance(&self, request: ListNetworkInstanceRequest) -> anyhow::Result<ListNetworkInstanceResponse>`

列出网络实例。

##### `delete_network_instance(&self, request: DeleteNetworkInstanceRequest) -> anyhow::Result<DeleteNetworkInstanceResponse>`

删除网络实例。

##### `get_network_instance_config(&self, request: GetNetworkInstanceConfigRequest) -> anyhow::Result<GetNetworkInstanceConfigResponse>`

获取网络实例配置。

##### `list_network_instance_meta(&self, request: ListNetworkInstanceMetaRequest) -> anyhow::Result<ListNetworkInstanceMetaResponse>`

列出网络实例元数据。

##### `list_instances(&self, request: ListNetworkInstanceRequest) -> anyhow::Result<ListNetworkInstanceResponse>`

列出所有网络实例（`list_network_instance` 的别名）。

##### `run_instance(&self, request: RunNetworkInstanceRequest) -> anyhow::Result<RunNetworkInstanceResponse>`

运行网络实例（`run_network_instance` 的别名）。

##### `delete_instance(&self, request: DeleteNetworkInstanceRequest) -> anyhow::Result<DeleteNetworkInstanceResponse>`

删除网络实例（`delete_network_instance` 的别名）。

##### `get_instance_config(&self, request: GetNetworkInstanceConfigRequest) -> anyhow::Result<GetNetworkInstanceConfigResponse>`

获取网络实例配置（`get_network_instance_config` 的别名）。

### 12. PeerCenterClient

对等节点中心客户端。

#### 方法

##### `report_peers(&self, request: ReportPeersRequest) -> anyhow::Result<ReportPeersResponse>`

报告对等节点。

##### `get_global_peer_map(&self, request: GetGlobalPeerMapRequest) -> anyhow::Result<GetGlobalPeerMapResponse>`

获取全局对等节点映射。

### 13. CredentialManageClient

凭证管理客户端。

#### 方法

##### `list_credential(&self, request: ListCredentialsRequest) -> anyhow::Result<ListCredentialsResponse>`

列出所有凭证。

##### `generate_credential(&self, request: GenerateCredentialRequest) -> anyhow::Result<GenerateCredentialResponse>`

生成凭证。

##### `revoke_credential(&self, request: RevokeCredentialRequest) -> anyhow::Result<RevokeCredentialResponse>`

撤销凭证。

## 错误类型

`exvlan-rpc` 使用 `RpcError` 作为统一的错误类型，定义在 `exvlan_rpc::error::RpcError`。

### RpcError 变体

- `ConnectionError(String)` - 连接错误
- `RpcResponseError(String)` - RPC 响应错误
- `TimeoutError` - 超时错误
- `SerializationError(String)` - 序列化错误
- `AddrParse(AddrParseError)` - 地址解析错误
- `Io(io::Error)` - IO 错误

## 类型定义

### PeerId

对等节点 ID，定义在 `exvlan::common::PeerId`。

### 协议缓冲区类型

所有的请求和响应类型都定义在 `exvlan::proto` 模块中。

## 示例

```rust
use exvlan::common::PeerId;
use exvlan_rpc::client::RpcClient;
use exvlan_rpc::error::RpcError;

#[tokio::main]
async fn main() -> Result<(), RpcError> {
    let client = RpcClient::new(
        "127.0.0.1:50051",
        PeerId::from(0),
        PeerId::from(1),
        "example.com".to_string(),
    ).await?;

    let from_peer_id = PeerId::from(0);
    let to_peer_id = PeerId::from(1);
    let domain_name = "example.com".to_string();

    let peer_manage = client.peer_manage(from_peer_id, to_peer_id, domain_name.clone());
    let stats = client.stats(from_peer_id, to_peer_id, domain_name.clone());
    let config = client.config(from_peer_id, to_peer_id, domain_name);

    Ok(())
}
```
