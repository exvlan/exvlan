# RPC服务模块

## 模块概述

RPC服务模块是 exvlan 的核心组件之一，负责提供各种远程过程调用（RPC）服务，用于管理网络实例、对等节点、连接器、端口转发等功能。该模块是系统的控制接口，允许用户和其他组件通过 RPC 调用与系统进行交互。

## 目录结构

```
exvlan/core/src/rpc_service/
├── mod.rs              # RPC服务模块入口
├── acl_manage.rs       # ACL管理服务
├── api.rs              # API服务
├── config.rs           # 配置服务
├── connector_manage.rs # 连接器管理服务
├── credential_manage.rs # 凭证管理服务
├── instance_manage.rs  # 实例管理服务
├── logger.rs           # 日志服务
├── mapped_listener_manage.rs # 映射监听器管理服务
├── peer_center.rs      # 对等节点中心服务
├── peer_manage.rs      # 对等节点管理服务
├── port_forward_manage.rs # 端口转发管理服务
├── protected_port.rs   # 受保护端口服务
├── proxy.rs            # 代理服务
├── remote_client.rs    # 远程客户端
├── stats.rs            # 统计信息服务
└── vpn_portal.rs       # VPN门户服务
```

## 主要组件

### RPC服务模块入口 (mod.rs)

**功能**：RPC服务模块的入口点，导出主要的RPC服务相关类型和功能

**主要导出**：
- `InstanceRpcService` 特质：实例RPC服务接口
- 各种RPC服务实现

### 实例管理服务 (instance_manage.rs)

**功能**：提供实例管理的RPC服务

**主要功能**：
- 创建、启动、停止、配置实例
- 列出实例
- 获取实例状态

### 对等节点管理服务 (peer_manage.rs)

**功能**：提供对等节点管理的RPC服务

**主要功能**：
- 添加、移除、管理对等节点
- 列出对等节点
- 获取对等节点状态

### 连接器管理服务 (connector_manage.rs)

**功能**：提供连接器管理的RPC服务

**主要功能**：
- 添加、移除、管理连接器
- 列出连接器
- 获取连接器状态

### 端口转发管理服务 (port_forward_manage.rs)

**功能**：提供端口转发管理的RPC服务

**主要功能**：
- 添加、移除、管理端口转发规则
- 列出端口转发规则
- 获取端口转发状态

### ACL管理服务 (acl_manage.rs)

**功能**：提供ACL管理的RPC服务

**主要功能**：
- 添加、移除、管理ACL规则
- 列出ACL规则
- 获取ACL状态

### 凭证管理服务 (credential_manage.rs)

**功能**：提供凭证管理的RPC服务

**主要功能**：
- 添加、移除、管理凭证
- 列出凭证
- 验证凭证

### 统计信息服务 (stats.rs)

**功能**：提供统计信息的RPC服务

**主要功能**：
- 获取系统统计信息
- 获取实例统计信息
- 获取对等节点统计信息

### VPN门户服务 (vpn_portal.rs)

**功能**：提供VPN门户的RPC服务

**主要功能**：
- 管理VPN门户配置
- 获取VPN门户状态
- 控制VPN门户服务

### 代理服务 (proxy.rs)

**功能**：提供代理服务的RPC服务

**主要功能**：
- 管理代理配置
- 获取代理状态
- 控制代理服务

### 日志服务 (logger.rs)

**功能**：提供日志管理的RPC服务

**主要功能**：
- 配置日志级别
- 获取日志配置
- 控制日志服务

### 映射监听器管理服务 (mapped_listener_manage.rs)

**功能**：提供映射监听器管理的RPC服务

**主要功能**：
- 添加、移除、管理映射监听器
- 列出映射监听器
- 获取映射监听器状态

### 对等节点中心服务 (peer_center.rs)

**功能**：提供对等节点中心的RPC服务

**主要功能**：
- 管理对等节点注册
- 处理对等节点发现
- 维护对等节点目录

### 配置服务 (config.rs)

**功能**：提供配置管理的RPC服务

**主要功能**：
- 获取和设置配置
- 验证配置
- 保存配置

### 受保护端口服务 (protected_port.rs)

**功能**：提供受保护端口管理的RPC服务

**主要功能**：
- 管理受保护端口
- 检查端口保护状态

### 远程客户端 (remote_client.rs)

**功能**：提供远程客户端的实现

**主要功能**：
- 连接到远程RPC服务
- 发送RPC请求
- 处理RPC响应

## 核心功能

### RPC服务管理

RPC服务模块负责管理各种RPC服务的生命周期：
- 注册和注销RPC服务
- 处理RPC请求
- 发送RPC响应
- 管理RPC连接

### 实例管理

RPC服务模块提供了完整的实例管理功能：
- 创建和配置实例
- 启动和停止实例
- 监控实例状态
- 管理实例资源

### 对等节点管理

RPC服务模块提供了完整的对等节点管理功能：
- 添加和移除对等节点
- 监控对等节点状态
- 管理对等节点连接
- 处理对等节点认证

### 连接器管理

RPC服务模块提供了完整的连接器管理功能：
- 添加和移除连接器
- 配置连接器参数
- 监控连接器状态
- 管理连接器生命周期

### 端口转发管理

RPC服务模块提供了完整的端口转发管理功能：
- 添加和移除端口转发规则
- 配置端口转发参数
- 监控端口转发状态
- 管理端口转发生命周期

### ACL管理

RPC服务模块提供了完整的ACL管理功能：
- 添加和移除ACL规则
- 配置ACL参数
- 监控ACL状态
- 管理ACL生命周期

### 凭证管理

RPC服务模块提供了完整的凭证管理功能：
- 添加和移除凭证
- 验证凭证
- 监控凭证状态
- 管理凭证生命周期

### 统计信息

RPC服务模块提供了完整的统计信息功能：
- 收集系统统计信息
- 收集实例统计信息
- 收集对等节点统计信息
- 提供统计信息查询

## 接口说明

### 实例RPC服务 (InstanceRpcService) [../src/rpc_service/mod.rs]

**功能**：定义实例RPC服务的接口

**主要方法**：
- [`get_peer_manage_service`](../src/rpc_service/mod.rs#L22)：获取对等节点管理服务
  - **参数**：无
  - **返回值**：&dyn PeerManageRpc - 对等节点管理服务
- [`get_connector_manage_service`](../src/rpc_service/mod.rs#L27)：获取连接器管理服务
  - **参数**：无
  - **返回值**：&dyn ConnectorManageRpc - 连接器管理服务
- [`get_mapped_listener_manage_service`](../src/rpc_service/mod.rs#L32)：获取映射监听器管理服务
  - **参数**：无
  - **返回值**：&dyn MappedListenerManageRpc - 映射监听器管理服务
- [`get_vpn_portal_service`](../src/rpc_service/mod.rs#L37)：获取VPN门户服务
  - **参数**：无
  - **返回值**：&dyn VpnPortalRpc - VPN门户服务
- [`get_proxy_service`](../src/rpc_service/mod.rs#L42)：获取代理服务
  - **参数**：无
  - **返回值**：&dyn ProxyRpc - 代理服务
- [`get_acl_manage_service`](../src/rpc_service/mod.rs#L53)：获取ACL管理服务
  - **参数**：无
  - **返回值**：&dyn AclManageRpc - ACL管理服务
- [`get_port_forward_manage_service`](../src/rpc_service/mod.rs#L58)：获取端口转发管理服务
  - **参数**：无
  - **返回值**：&dyn PortForwardManageRpc - 端口转发管理服务
- [`get_stats_service`](../src/rpc_service/mod.rs#L63)：获取统计信息服务
  - **参数**：无
  - **返回值**：&dyn StatsRpc - 统计信息服务
- [`get_config_service`](../src/rpc_service/mod.rs#L68)：获取配置服务
  - **参数**：无
  - **返回值**：&dyn ConfigRpc - 配置服务
- [`get_peer_center_service`](../src/rpc_service/mod.rs#L73)：获取对等节点中心服务
  - **参数**：无
  - **返回值**：&dyn PeerCenterRpc - 对等节点中心服务
- [`get_credential_manage_service`](../src/rpc_service/mod.rs#L81)：获取凭证管理服务
  - **参数**：无
  - **返回值**：&dyn CredentialManageRpc - 凭证管理服务

### 实例管理服务 (InstanceManageService) [../src/rpc_service/instance_manage.rs]

**功能**：提供实例管理的RPC服务

**主要方法**：
- [`new`](../src/rpc_service/instance_manage.rs#L20)：创建实例管理服务
  - **参数**：
    - `manager: Arc<NetworkInstanceManager>`：网络实例管理器
    - `hooks: Arc<dyn WebClientHooks>`：Web客户端钩子
  - **返回值**：`Self` - 新创建的实例管理服务
- [`validate_config`](../src/rpc_service/instance_manage.rs#L29)：验证配置
  - **参数**：
    - `request: ValidateConfigRequest`：验证配置请求
  - **返回值**：`Result<ValidateConfigResponse, Error>` - 验证配置响应
- [`run_network_instance`](../src/rpc_service/instance_manage.rs#L38)：运行网络实例
  - **参数**：
    - `request: RunNetworkInstanceRequest`：运行网络实例请求
  - **返回值**：`Result<RunNetworkInstanceResponse, Error>` - 运行网络实例响应
- [`list_network_instance`](../src/rpc_service/instance_manage.rs#L171)：列出网络实例
  - **参数**：
    - `request: ListNetworkInstanceRequest`：列出网络实例请求
  - **返回值**：`Result<ListNetworkInstanceResponse, Error>` - 列出网络实例响应
- [`delete_network_instance`](../src/rpc_service/instance_manage.rs#L187)：删除网络实例
  - **参数**：
    - `request: DeleteNetworkInstanceRequest`：删除网络实例请求
  - **返回值**：`Result<DeleteNetworkInstanceResponse, Error>` - 删除网络实例响应
- [`get_network_instance_config`](../src/rpc_service/instance_manage.rs#L232)：获取网络实例配置
  - **参数**：
    - `request: GetNetworkInstanceConfigRequest`：获取网络实例配置请求
  - **返回值**：`Result<GetNetworkInstanceConfigResponse, Error>` - 获取网络实例配置响应

### 对等节点管理服务 (PeerManageService) [../src/rpc_service/peer_manage.rs]

**功能**：提供对等节点管理的RPC服务

**主要方法**：
- [`new`](../src/rpc_service/peer_manage.rs#L17)：创建对等节点管理服务
  - **参数**：
    - `instance_manager: Arc<NetworkInstanceManager>`：网络实例管理器
  - **返回值**：`Self` - 新创建的对等节点管理服务
- [`list_peer`](../src/rpc_service/peer_manage.rs#L26)：列出对等节点
  - **参数**：
    - `request: ListPeerRequest`：列出对等节点请求
  - **返回值**：`Result<ListPeerResponse, Error>` - 列出对等节点响应
- [`list_route`](../src/rpc_service/peer_manage.rs#L37)：列出路由
  - **参数**：
    - `request: ListRouteRequest`：列出路由请求
  - **返回值**：`Result<ListRouteResponse, Error>` - 列出路由响应
- [`dump_route`](../src/rpc_service/peer_manage.rs#L48)：导出路由
  - **参数**：
    - `request: DumpRouteRequest`：导出路由请求
  - **返回值**：`Result<DumpRouteResponse, Error>` - 导出路由响应
- [`list_foreign_network`](../src/rpc_service/peer_manage.rs#L59)：列出外部网络
  - **参数**：
    - `request: ListForeignNetworkRequest`：列出外部网络请求
  - **返回值**：`Result<ListForeignNetworkResponse, Error>` - 列出外部网络响应

### 连接器管理服务 (ConnectorManageService) [../src/rpc_service/connector_manage.rs]

**功能**：提供连接器管理的RPC服务

**主要方法**：
- [`new`](../src/rpc_service/connector_manage.rs#L17)：创建连接器管理服务
  - **参数**：
    - `instance_manager: Arc<NetworkInstanceManager>`：网络实例管理器
  - **返回值**：`Self` - 新创建的连接器管理服务
- [`list_connector`](../src/rpc_service/connector_manage.rs#L26)：列出连接器
  - **参数**：
    - `request: ListConnectorRequest`：列出连接器请求
  - **返回值**：`Result<ListConnectorResponse, Error>` - 列出连接器响应

### 端口转发管理服务 (PortForwardManageService) [../src/rpc_service/port_forward_manage.rs]

**功能**：提供端口转发管理的RPC服务

**主要方法**：
- [`new`](../src/rpc_service/port_forward_manage.rs#L17)：创建端口转发管理服务
  - **参数**：
    - `instance_manager: Arc<NetworkInstanceManager>`：网络实例管理器
  - **返回值**：`Self` - 新创建的端口转发管理服务
- [`list_port_forward`](../src/rpc_service/port_forward_manage.rs#L26)：列出端口转发
  - **参数**：
    - `request: ListPortForwardRequest`：列出端口转发请求
  - **返回值**：`Result<ListPortForwardResponse, Error>` - 列出端口转发响应

### ACL管理服务 (AclManageService) [../src/rpc_service/acl_manage.rs]

**功能**：提供ACL管理的RPC服务

**主要方法**：
- [`new`](../src/rpc_service/acl_manage.rs#L20)：创建ACL管理服务
  - **参数**：
    - `instance_manager: Arc<NetworkInstanceManager>`：网络实例管理器
  - **返回值**：`Self` - 新创建的ACL管理服务
- [`get_acl_stats`](../src/rpc_service/acl_manage.rs#L29)：获取ACL统计
  - **参数**：
    - `request: GetAclStatsRequest`：获取ACL统计请求
  - **返回值**：`Result<GetAclStatsResponse, Error>` - 获取ACL统计响应
- [`get_whitelist`](../src/rpc_service/acl_manage.rs#L40)：获取白名单
  - **参数**：
    - `request: GetWhitelistRequest`：获取白名单请求
  - **返回值**：`Result<GetWhitelistResponse, Error>` - 获取白名单响应

### 凭证管理服务 (CredentialManageService) [../src/rpc_service/credential_manage.rs]

**功能**：提供凭证管理的RPC服务

**主要方法**：
- [`new`](../src/rpc_service/credential_manage.rs#L21)：创建凭证管理服务
  - **参数**：
    - `instance_manager: Arc<NetworkInstanceManager>`：网络实例管理器
  - **返回值**：`Self` - 新创建的凭证管理服务
- [`generate_credential`](../src/rpc_service/credential_manage.rs#L30)：生成凭证
  - **参数**：
    - `request: GenerateCredentialRequest`：生成凭证请求
  - **返回值**：`Result<GenerateCredentialResponse, Error>` - 生成凭证响应
- [`revoke_credential`](../src/rpc_service/credential_manage.rs#L41)：撤销凭证
  - **参数**：
    - `request: RevokeCredentialRequest`：撤销凭证请求
  - **返回值**：`Result<RevokeCredentialResponse, Error>` - 撤销凭证响应
- [`list_credentials`](../src/rpc_service/credential_manage.rs#L52)：列出凭证
  - **参数**：
    - `request: ListCredentialsRequest`：列出凭证请求
  - **返回值**：`Result<ListCredentialsResponse, Error>` - 列出凭证响应

### 统计信息服务 (StatsService)

**功能**：提供统计信息的RPC服务

**主要方法**：
- `get_system_stats`：获取系统统计信息
- `get_instance_stats`：获取实例统计信息
- `get_peer_stats`：获取对等节点统计信息

## 使用示例

### 创建和管理实例

```rust
// 创建实例管理服务客户端
let instance_manage_client = InstanceManageClient::new(channel);

// 创建实例
let request = CreateInstanceRequest {
    name: "my-instance".to_string(),
    ipv4: Some("10.0.0.1/24".parse()?),
    ..Default::default()
};
let response = instance_manage_client.create_instance(request).await?;

// 启动实例
let request = StartInstanceRequest {
    instance_id: response.instance_id,
};
let response = instance_manage_client.start_instance(request).await?;

// 列出实例
let request = ListInstancesRequest {};
let response = instance_manage_client.list_instances(request).await?;
```

### 管理对等节点

```rust
// 创建对等节点管理服务客户端
let peer_manage_client = PeerManageClient::new(channel);

// 添加对等节点
let request = AddPeerRequest {
    instance_id: "instance-1".to_string(),
    address: "192.168.1.100:8080".to_string(),
    credential: Some(credential),
};
let response = peer_manage_client.add_peer(request).await?;

// 列出对等节点
let request = ListPeersRequest {
    instance_id: "instance-1".to_string(),
};
let response = peer_manage_client.list_peers(request).await?;
```

### 配置端口转发

```rust
// 创建端口转发管理服务客户端
let port_forward_client = PortForwardManageClient::new(channel);

// 添加端口转发规则
let request = AddPortForwardRequest {
    instance_id: "instance-1".to_string(),
    local_port: 8080,
    remote_addr: "10.0.0.2:80".to_string(),
};
let response = port_forward_client.add_port_forward(request).await?;

// 列出端口转发规则
let request = ListPortForwardsRequest {
    instance_id: "instance-1".to_string(),
};
let response = port_forward_client.list_port_forwards(request).await?;
```

## 注意事项

1. **安全性**：RPC服务模块需要确保通信的安全性：
   - 使用加密通信
   - 实现身份验证
   - 授权访问控制

2. **性能**：RPC服务模块的性能对整个系统的影响很大，需要注意优化：
   - 合理设置RPC超时
   - 优化序列化和反序列化
   - 合理使用并发

3. **错误处理**：RPC服务模块需要处理各种异常情况：
   - 网络故障
   - 服务不可用
   - 权限不足

4. **兼容性**：确保RPC服务模块与不同版本的客户端兼容：
   - 版本协商机制
   - 向后兼容

5. **监控**：实现适当的监控机制，及时发现和解决问题：
   - RPC调用监控
   - 错误监控
   - 性能监控

6. **资源管理**：确保RPC服务模块的资源管理正确：
   - 及时释放不需要的连接
   - 合理管理内存使用
   - 避免资源泄漏