# 实例管理模块

## 模块概述

实例管理模块是 exvlan 的核心组件之一，负责管理网络实例的生命周期，包括创建、启动、停止、配置等功能。该模块还提供了DNS服务器、监听器、代理CIDR监控等功能，确保网络实例能够正常运行。

## 目录结构

```
exvlan/core/src/instance/
├── mod.rs              # 实例管理模块入口
├── dns_server/        # DNS服务器
│   ├── mod.rs
│   ├── client_instance.rs
│   ├── config.rs
│   ├── runner.rs
│   ├── server.rs
│   ├── server_instance.rs
│   ├── system_config/   # 系统配置
│   └── tests.rs
├── instance.rs         # 实例实现
├── listeners.rs        # 监听器
├── proxy_cidrs_monitor.rs # 代理CIDR监控
└── virtual_nic.rs      # 虚拟网卡
```

## 主要组件

### 实例管理模块入口 (mod.rs)

**功能**：实例管理模块的入口点，导出主要的实例相关类型和功能

**主要导出**：
- 实例相关的类型和函数
- DNS服务器相关的类型和函数
- 监听器相关的类型和函数

### 实例实现 (instance.rs)

**功能**：实现网络实例的核心功能

**主要组件**：
- `IpProxy` 结构体：IP代理实现
- `Instance` 结构体：网络实例实现
- 实例的启动、停止、配置等方法

### DNS服务器 (dns_server/)

**功能**：提供DNS服务器功能，用于虚拟网络的域名解析

**主要组件**：
- `DnsServer` 结构体：DNS服务器实现
- `DnsServerInstance` 结构体：DNS服务器实例
- `DnsClientInstance` 结构体：DNS客户端实例
- 系统配置适配：支持不同系统的DNS配置

### 监听器 (listeners.rs)

**功能**：管理网络监听器，用于接受连接

**主要功能**：
- 创建和管理监听器
- 处理监听器的启动和停止
- 配置监听器的参数

### 代理CIDR监控 (proxy_cidrs_monitor.rs)

**功能**：监控和更新代理CIDR，用于控制代理范围

**主要功能**：
- 监控CIDR配置的变化
- 更新代理CIDR列表
- 提供CIDR查询功能

### 虚拟网卡 (virtual_nic.rs)

**功能**：管理虚拟网络接口，用于虚拟网络的通信

**主要功能**：
- 创建和管理虚拟网卡
- 配置虚拟网卡的IP地址和子网
- 处理虚拟网卡的启动和停止

## 核心功能

### 实例生命周期管理

实例管理模块负责网络实例的完整生命周期管理：
- 创建实例：初始化实例配置和资源
- 启动实例：启动实例的各种服务
- 停止实例：停止实例的所有服务
- 销毁实例：清理实例的所有资源

### DNS服务

实例管理模块提供了完整的DNS服务：
- 自定义DNS服务器：为虚拟网络提供域名解析
- DNS客户端配置：自动配置系统DNS设置
- 域名解析：支持虚拟网络内的域名解析
- 系统配置适配：适配不同系统的DNS配置

### 监听器管理

实例管理模块管理各种网络监听器：
- TCP监听器：接受TCP连接
- UDP监听器：接受UDP连接
- WebSocket监听器：接受WebSocket连接
- 监听器配置：配置监听器的参数和行为

### 代理CIDR管理

实例管理模块管理代理CIDR：
- CIDR监控：监控CIDR配置的变化
- CIDR更新：实时更新代理CIDR列表
- CIDR查询：提供CIDR查询功能
- 代理范围控制：通过CIDR控制代理范围

### 虚拟网卡管理

实例管理模块管理虚拟网卡：
- 虚拟网卡创建：创建虚拟网络接口
- 虚拟网卡配置：配置IP地址和子网
- 虚拟网卡启动：启动虚拟网卡
- 虚拟网卡停止：停止虚拟网卡

## 接口说明

### 实例 (Instance) [../src/instance/instance.rs]

**功能**：表示一个网络实例

**主要方法**：
- [`new`](../src/instance/instance.rs#L636-L648)：创建新的网络实例
  - **参数**：无（通过构建器模式创建）
  - **返回值**：`Self` - 新创建的网络实例
- [`run`](../src/instance/instance.rs#L930-L1024)：运行网络实例
  - **参数**：无
  - **返回值**：`Result<(), Error>` - 运行结果
- [`run_ip_proxy`](../src/instance/instance.rs#L1026-L1031)：运行IP代理
  - **参数**：无
  - **返回值**：`Result<(), Error>` - 运行结果
- [`run_vpn_portal`](../src/instance/instance.rs#L1034-L1045)：运行VPN门户
  - **参数**：无
  - **返回值**：`Result<(), Error>` - 运行结果
- [`get_peer_manager`](../src/instance/instance.rs#L1046-L1048)：获取对等节点管理器
  - **参数**：无
  - **返回值**：`Arc<PeerManager>` - 对等节点管理器
- [`close_peer_conn`](../src/instance/instance.rs#L1050-L1061)：关闭对等节点连接
  - **参数**：
    - `peer_id: PeerId`：对等节点ID
    - `conn_id: &PeerConnId`：连接ID
  - **返回值**：`Result<(), Error>` - 关闭结果
- [`wait`](../src/instance/instance.rs#L1062-L1064)：等待实例结束
  - **参数**：无
  - **返回值**：无
- [`id`](../src/instance/instance.rs#L1066-L1068)：获取实例ID
  - **参数**：无
  - **返回值**：`uuid::Uuid` - 实例ID
- [`peer_id`](../src/instance/instance.rs#L1070-L1072)：获取对等节点ID
  - **参数**：无
  - **返回值**：`PeerId` - 对等节点ID
- [`get_config_patcher`](../src/instance/instance.rs#L1238-L1250)：获取配置补丁器
  - **参数**：无
  - **返回值**：`InstanceConfigPatcher` - 配置补丁器
- [`get_api_rpc_service`](../src/instance/instance.rs#L1291-L1299)：获取API RPC服务
  - **参数**：无
  - **返回值**：`impl InstanceRpcService` - API RPC服务

### IP代理 (IpProxy) [../src/instance/instance.rs]

**功能**：实现IP代理功能

**主要方法**：
- [`new`](../src/instance/instance.rs#L82-L95)：创建新的IP代理
  - **参数**：
    - `global_ctx: ArcGlobalCtx`：全局上下文
    - `peer_manager: Arc<PeerManager>`：对等节点管理器
  - **返回值**：`Result<Self, Error>` - 新创建的IP代理
- [`start`](../src/instance/instance.rs#L97-L130)：启动IP代理
  - **参数**：无
  - **返回值**：`Result<(), Error>` - 启动结果

### DNS服务器 (Server) [../src/instance/dns_server/server.rs]

**功能**：实现DNS服务器功能

**主要方法**：
- [`new`](../src/instance/dns_server/server.rs#L79-L81)：创建新的DNS服务器
  - **参数**：
    - `config: RunConfig`：DNS服务器配置
  - **返回值**：`Self` - 新创建的DNS服务器
- [`try_new`](../src/instance/dns_server/server.rs#L83-L130)：尝试创建新的DNS服务器
  - **参数**：
    - `config: RunConfig`：DNS服务器配置
  - **返回值**：`Result<Self>` - 新创建的DNS服务器
- [`udp_local_addr`](../src/instance/dns_server/server.rs#L132-L134)：获取UDP本地地址
  - **参数**：无
  - **返回值**：`Option<SocketAddr>` - UDP本地地址
- [`tcp_local_addr`](../src/instance/dns_server/server.rs#L136-L138)：获取TCP本地地址
  - **参数**：无
  - **返回值**：`Option<SocketAddr>` - TCP本地地址
- [`register_udp_socket`](../src/instance/dns_server/server.rs#L140-L181)：注册UDP套接字
  - **参数**：
    - `address: String`：地址字符串
  - **返回值**：`Result<SocketAddr>` - 本地地址
- [`run`](../src/instance/dns_server/server.rs#L183-L199)：运行DNS服务器
  - **参数**：无
  - **返回值**：`Result<()>` - 运行结果
- [`shutdown`](../src/instance/dns_server/server.rs#L201-L205)：关闭DNS服务器
  - **参数**：无
  - **返回值**：`Result<()>` - 关闭结果
- [`upsert`](../src/instance/dns_server/server.rs#L206-L209)：插入或更新授权
  - **参数**：
    - `name: LowerName`：域名
    - `authority: Arc<dyn AuthorityObject>`：授权对象
  - **返回值**：无
- [`remove`](../src/instance/dns_server/server.rs#L210-L213)：移除授权
  - **参数**：
    - `name: &LowerName`：域名
  - **返回值**：`Option<Vec<Arc<dyn AuthorityObject>>>` - 被移除的授权对象
- [`contains`](../src/instance/dns_server/server.rs#L227-L230)：检查是否包含域名
  - **参数**：
    - `name: &LowerName`：域名
  - **返回值**：`bool` - 是否包含

### 监听器管理器 (ListenerManager) [../src/instance/listeners.rs]

**功能**：管理网络监听器

**主要方法**：
- [`new`](../src/instance/listeners.rs#L106-L114)：创建新的监听器管理器
  - **参数**：
    - `global_ctx: ArcGlobalCtx`：全局上下文
    - `peer_manager: Arc<H>`：对等节点管理器
  - **返回值**：`Self` - 新创建的监听器管理器
- [`prepare_listeners`](../src/instance/listeners.rs#L116-L165)：准备监听器
  - **参数**：无
  - **返回值**：`Result<(), Error>` - 准备结果
- [`add_listener`](../src/instance/listeners.rs#L167-L177)：添加监听器
  - **参数**：
    - `creator: C`：监听器创建函数
    - `must_succ: bool`：是否必须成功
  - **返回值**：`Result<(), Error>` - 添加结果
- [`run`](../src/instance/listeners.rs#L258-L297)：运行监听器
  - **参数**：无
  - **返回值**：`Result<(), Error>` - 运行结果

### 公共函数

- [`create_listener_by_url`](../src/instance/listeners.rs#L26-L59)：通过URL创建监听器
  - **参数**：
    - `l: &url::Url`：URL
    - `global_ctx: ArcGlobalCtx`：全局上下文
  - **返回值**：`Result<Box<dyn TunnelListener>, Error>` - 监听器
- [`is_url_host_ipv6`](../src/instance/listeners.rs#L61-L63)：检查URL主机是否为IPv6
  - **参数**：
    - `l: &url::Url`：URL
  - **返回值**：`bool` - 是否为IPv6
- [`is_url_host_unspecified`](../src/instance/listeners.rs#L65-L71)：检查URL主机是否未指定
  - **参数**：
    - `l: &url::Url`：URL
  - **返回值**：`bool` - 是否未指定

## 使用示例

### 创建和启动网络实例

```rust
// 创建网络实例
let instance = Instance::new(config).await?;

// 运行实例
instance.run().await?;

// 等待实例结束
instance.wait().await?;
```

### 配置DNS服务器

```rust
// 创建DNS服务器实例
let dns_server = DnsServerInstance::new(config).await?;

// 启动DNS服务器
dns_server.start().await?;

// 添加DNS记录
dns_server.add_record("example.com", "10.0.0.1").await?;

// 停止DNS服务器
dns_server.stop().await?;
```

### 配置监听器

```rust
// 创建监听器管理器
let listeners = Listeners::new();

// 添加TCP监听器
listeners.add_listener(ListenerType::Tcp, "0.0.0.0:8080").await?;

// 添加UDP监听器
listeners.add_listener(ListenerType::Udp, "0.0.0.0:8081").await?;

// 获取所有监听器
let all_listeners = listeners.get_listeners().await?;
```

### 配置代理CIDR

```rust
// 创建代理CIDR监控
let cidrs_monitor = ProxyCidrsMonitor::new(config).await?;

// 启动监控
cidrs_monitor.start().await?;

// 检查IP是否在代理范围内
let is_proxied = cidrs_monitor.contains_ip(ip_address).await?;

// 停止监控
cidrs_monitor.stop().await?;
```

## 注意事项

1. **资源管理**：实例管理模块需要管理大量资源，包括网络接口、监听器、DNS服务器等，需要确保资源的正确分配和释放。

2. **系统兼容性**：不同系统的网络配置和DNS设置方式不同，需要确保实例管理模块在不同系统上都能正常工作。

3. **性能优化**：实例管理模块的性能对整个系统的影响很大，需要注意优化：
   - 合理设置监听器的参数
   - 优化DNS服务器的响应时间
   - 合理使用缓存

4. **错误处理**：实例管理模块需要处理各种异常情况：
   - 网络故障
   - 资源不足
   - 配置错误

5. **安全性**：确保实例管理模块的安全性：
   - 正确配置防火墙规则
   - 限制访问权限
   - 加密敏感数据

6. **监控**：实现适当的监控机制，及时发现和解决问题：
   - 实例状态监控
   - 服务状态监控
   - 错误监控