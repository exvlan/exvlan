# 对等节点模块

## 模块概述

对等节点模块是 exvlan 的核心组件之一，负责管理网络中的对等节点，包括连接建立、会话管理、路由计算、加密通信等功能。该模块是实现分布式虚拟网络的关键，确保对等节点之间能够安全、高效地通信。

## 目录结构

```
exvlan/core/src/peers/
├── mod.rs              # 对等节点模块入口
├── acl_filter.rs       # ACL过滤器
├── credential_manager.rs # 凭证管理器
├── encrypt/            # 加密实现
│   ├── mod.rs
│   ├── aes_gcm.rs
│   ├── openssl.rs
│   ├── ring.rs
│   └── xor.rs
├── foreign_network_client.rs # 外部网络客户端
├── foreign_network_manager.rs # 外部网络管理器
├── graph_algo.rs       # 图算法
├── peer.rs             # 对等节点实现
├── peer_conn.rs        # 对等节点连接
├── peer_conn_ping.rs   # 对等节点连接ping
├── peer_manager.rs     # 对等节点管理器
├── peer_map.rs         # 对等节点映射
├── peer_ospf_route.rs  # OSPF路由
├── peer_rpc.rs         # 对等节点RPC
├── peer_rpc_service.rs # 对等节点RPC服务
├── peer_session.rs     # 对等节点会话
├── peer_task.rs        # 对等节点任务
├── relay_peer_map.rs   # 中继对等节点映射
├── rpc_service.rs      # RPC服务
├── route_trait.rs      # 路由特质
├── tests.rs            # 测试
└── traffic_metrics.rs  # 流量指标
```

## 主要组件

### 对等节点模块入口 (mod.rs)

**功能**：对等节点模块的入口点，导出主要的对等节点相关类型和功能

**主要导出**：
- `PeerPacketFilter` 特质：对等节点数据包过滤器
- `NicPacketFilter` 特质：NIC数据包过滤器
- 数据包通道类型和函数

### 对等节点管理器 (peer_manager.rs)

**功能**：管理所有对等节点的连接和状态

**主要功能**：
- 管理对等节点的生命周期
- 处理对等节点的加入和离开
- 维护对等节点的连接状态
- 提供对等节点的查找和访问

### 对等节点连接 (peer_conn.rs)

**功能**：管理对等节点之间的连接

**主要功能**：
- 建立和维护对等节点连接
- 处理连接的心跳检测
- 处理连接的错误和重连
- 管理连接的加密和认证

### 对等节点会话 (peer_session.rs)

**功能**：管理对等节点之间的会话

**主要功能**：
- 处理对等节点的会话状态
- 管理会话的加密和认证
- 处理会话的建立和关闭

### 对等节点路由 (peer_ospf_route.rs)

**功能**：实现OSPF路由协议，用于对等节点之间的路由计算

**主要功能**：
- 计算最短路径
- 维护路由表
- 处理路由更新
- 支持动态路由

### 加密实现 (encrypt/)

**功能**：提供对等节点之间的加密通信

**主要实现**：
- AES-GCM：高级加密标准
- OpenSSL：开源加密库
- Ring：现代加密库
- XOR：简单异或加密

### ACL过滤器 (acl_filter.rs)

**功能**：实现访问控制列表，控制对等节点的访问权限

**主要功能**：
- 基于IP地址的访问控制
- 基于端口的访问控制
- 基于协议的访问控制

## 核心功能

### 对等节点发现

对等节点模块支持多种对等节点发现方式，包括：
- 静态配置：手动指定对等节点地址
- 动态发现：通过网络扫描或DNS服务发现
- 中继服务器：通过中继服务器发现其他对等节点

### 连接建立

对等节点模块支持多种连接建立方式，包括：
- 直接连接：直接连接到对等节点
- 中继连接：通过中继服务器连接到对等节点
- 打洞连接：通过UDP打洞技术建立连接

### 加密通信

对等节点模块提供了多种加密方式，确保通信的安全性：
- 对称加密：使用AES-GCM等算法
- 非对称加密：使用RSA等算法
- 密钥交换：使用Diffie-Hellman等算法

### 路由计算

对等节点模块实现了OSPF路由协议，用于计算最优路由：
- 最短路径优先
- 动态路由更新
- 路由故障检测和恢复

### 流量管理

对等节点模块提供了流量管理功能：
- 流量统计：统计发送和接收的流量
- 流量控制：控制流量速率
- 流量优化：优化流量路径

## 接口说明

### 对等节点管理器 (PeerManager) [../src/peers/peer_manager.rs]

**功能**：管理所有对等节点

**主要方法**：
- [`new`](../src/peers/peer_manager.rs#L233)：创建新的对等节点管理器
  - **参数**：
    - `global_ctx: ArcGlobalCtx`：全局上下文
    - `config: impl ConfigLoader`：配置加载器
  - **返回值**：`Self` - 新创建的对等节点管理器
- [`add_client_tunnel`](../src/peers/peer_manager.rs#L570)：添加客户端隧道
  - **参数**：
    - `tunnel: Box<dyn Tunnel>`：隧道
  - **返回值**：`Result<(), Error>` - 添加结果
- [`add_tunnel_as_server`](../src/peers/peer_manager.rs#L678)：作为服务器添加隧道
  - **参数**：
    - `tunnel: Box<dyn Tunnel>`：隧道
    - `require_handshake: bool`：是否需要握手
  - **返回值**：`Result<(), Error>` - 添加结果
- [`list_peers`](../src/peers/peer_manager.rs#L1170)：列出对等节点
  - **参数**：无
  - **返回值**：`Vec<PeerId>` - 对等节点ID列表
- [`close_peer`](../src/peers/peer_manager.rs#L1188)：关闭对等节点
  - **参数**：
    - `peer_id: PeerId`：对等节点ID
  - **返回值**：无

### 对等节点 (Peer) [../src/peers/peer.rs]

**功能**：表示一个对等节点

**主要方法**：
- [`new`](../src/peers/peer.rs#L51-L137)：创建新的对等节点
  - **参数**：
    - `peer_node_id: PeerId`：对等节点ID
    - `packet_recv_chan: PacketRecvChan`：数据包接收通道
    - `global_ctx: ArcGlobalCtx`：全局上下文
  - **返回值**：`Self` - 新创建的对等节点
- [`add_peer_conn`](../src/peers/peer.rs#L139)：添加对等节点连接
  - **参数**：
    - `conn: PeerConn`：对等节点连接
  - **返回值**：`Result<(), Error>` - 添加结果
- [`send_msg`](../src/peers/peer.rs#L213)：发送消息
  - **参数**：
    - `msg: ZCPacket`：数据包
  - **返回值**：`Result<(), Error>` - 发送结果
- [`close_peer_conn`](../src/peers/peer.rs#L222)：关闭对等节点连接
  - **参数**：
    - `conn_id: &PeerConnId`：连接ID
  - **返回值**：`Result<(), Error>` - 关闭结果
- [`list_peer_conns`](../src/peers/peer.rs#L231)：列出对等节点连接
  - **参数**：无
  - **返回值**：`Vec<PeerConnInfo>` - 连接信息列表
- [`has_live_conns`](../src/peers/peer.rs#L251)：检查是否有活动连接
  - **参数**：无
  - **返回值**：`bool` - 是否有活动连接
- [`get_peer_identity_type`](../src/peers/peer.rs#L273)：获取对等节点身份类型
  - **参数**：无
  - **返回值**：`Option<PeerIdentityType>` - 身份类型

### 对等节点连接 (PeerConn) [../src/peers/peer_conn.rs]

**功能**：表示对等节点之间的连接

**主要方法**：
- [`new`](../src/peers/peer_conn.rs#L333)：创建新的对等节点连接
  - **参数**：
    - `tunnel: Box<dyn Tunnel>`：隧道
    - `global_ctx: ArcGlobalCtx`：全局上下文
  - **返回值**：`Self` - 新创建的对等节点连接
- [`do_handshake_as_server`](../src/peers/peer_conn.rs#L1292)：作为服务器执行握手
  - **参数**：无
  - **返回值**：`Result<(), Error>` - 握手结果
- [`do_handshake_as_client`](../src/peers/peer_conn.rs#L1297)：作为客户端执行握手
  - **参数**：无
  - **返回值**：`Result<(), Error>` - 握手结果
- [`send_msg`](../src/peers/peer_conn.rs#L1463)：发送消息
  - **参数**：
    - `msg: ZCPacket`：数据包
  - **返回值**：`Result<(), Error>` - 发送结果
- [`get_peer_id`](../src/peers/peer_conn.rs#L1467)：获取对等节点ID
  - **参数**：无
  - **返回值**：`PeerId` - 对等节点ID
- [`get_conn_id`](../src/peers/peer_conn.rs#L434)：获取连接ID
  - **参数**：无
  - **返回值**：`PeerConnId` - 连接ID
- [`is_closed`](../src/peers/peer_conn.rs#L446)：检查是否已关闭
  - **参数**：无
  - **返回值**：`bool` - 是否已关闭

### 凭证管理器 (CredentialManager)

**功能**：管理对等节点的凭证

**主要方法**：
- `new`：创建新的凭证管理器
- `add_credential`：添加凭证
- `remove_credential`：移除凭证
- `get_credential`：获取凭证
- `validate_credential`：验证凭证

## 使用示例

### 创建对等节点管理器

```rust
// 创建对等节点管理器
let peer_manager = PeerManager::new(global_ctx).await?;

// 添加对等节点
let peer = peer_manager.add_peer(addr, credential).await?;

// 发送消息
let data = b"Hello, peer!";
peer_manager.send_msg_by_peer_id(peer_id, data).await?;

// 接收消息
let msg = peer_manager.recv_msg().await?;
```

### 配置对等节点路由

```rust
// 创建OSPF路由
let ospf_route = OspfRoute::new();

// 添加路由
ospf_route.add_route(destination, next_hop, cost).await?;

// 获取路由
let route = ospf_route.get_route(destination).await?;

// 更新路由
ospf_route.update_route(destination, next_hop, cost).await?;
```

### 配置ACL过滤器

```rust
// 创建ACL过滤器
let acl_filter = AclFilter::new();

// 添加允许规则
acl_filter.add_allow_rule(ip_address, port, protocol).await?;

// 添加拒绝规则
acl_filter.add_deny_rule(ip_address, port, protocol).await?;

// 检查访问权限
let allowed = acl_filter.check_access(source_ip, destination_ip, port, protocol).await?;
```

## 注意事项

1. **安全性**：确保对等节点之间的通信使用适当的加密方式，保护数据安全。

2. **性能**：对等节点模块的性能对整个系统的影响很大，需要注意优化：
   - 合理设置连接超时时间
   - 优化路由计算算法
   - 合理使用缓存

3. **可靠性**：确保对等节点模块的可靠性，处理各种异常情况：
   - 网络故障
   - 对等节点离线
   - 连接超时

4. **扩展性**：设计对等节点模块时需要考虑扩展性，支持大规模网络：
   - 合理的路由算法
   - 有效的资源管理
   - 适当的负载均衡

5. **兼容性**：确保对等节点模块与不同版本的协议兼容：
   - 版本协商机制
   - 向后兼容

6. **监控**：实现适当的监控机制，及时发现和解决问题：
   - 连接状态监控
   - 流量监控
   - 错误监控