# 隧道模块

## 模块概述

隧道模块是 exvlan 的核心组件之一，负责实现各种隧道协议，用于在不同网络环境中建立安全、可靠的通信通道。该模块支持多种隧道协议，包括 TCP、UDP、WebSocket、WireGuard、QUIC 等，以适应不同的网络环境和需求。

## 目录结构

```
exvlan/core/src/tunnel/
├── mod.rs              # 隧道模块入口
├── buf.rs              # 缓冲区实现
├── common.rs           # 公共功能
├── fake_tcp/           # 假TCP实现
│   ├── mod.rs
│   ├── stack.rs
│   ├── packet.rs
│   └── netfilter/      # 网络过滤器
├── filter.rs           # 过滤器实现
├── insecure_tls.rs     # 不安全TLS实现
├── mpsc.rs             # MPSC通道实现
├── packet_def.rs       # 数据包定义
├── quic.rs             # QUIC协议实现
├── ring.rs             # Ring协议实现
├── stats.rs            # 统计信息
├── tcp.rs              # TCP协议实现
├── udp.rs              # UDP协议实现
├── unix.rs             # Unix socket实现
├── websocket.rs        # WebSocket协议实现
└── wireguard.rs        # WireGuard协议实现
```

## 主要组件

### 隧道模块入口 (mod.rs)

**功能**：隧道模块的入口点，导出主要的隧道相关类型和功能

**主要导出**：
- `IpScheme` 枚举：定义不同的IP方案
- `Tunnel` 特质：隧道的基本接口
- `TunnelListener` 特质：隧道监听器接口
- `TunnelConnector` 特质：隧道连接器接口
- `TunnelError` 枚举：隧道错误类型
- `IpVersion` 枚举：IP版本
- 各种隧道实现

### 数据包定义 (packet_def.rs)

**功能**：定义隧道使用的数据包格式

**主要组件**：
- `PacketType` 枚举：数据包类型
- `PeerManagerHeader` 结构体：对等节点管理器头部
- `ZCPacket` 结构体：隧道数据包

### TCP 隧道 (tcp.rs)

**功能**：基于TCP协议的隧道实现

**主要特点**：
- 可靠的连接
- 适用于各种网络环境
- 支持TLS加密

### UDP 隧道 (udp.rs)

**功能**：基于UDP协议的隧道实现

**主要特点**：
- 低延迟
- 高吞吐量
- 适用于实时应用

### WebSocket 隧道 (websocket.rs)

**功能**：基于WebSocket协议的隧道实现

**主要特点**：
- 可穿透防火墙
- 支持HTTPS
- 适用于Web环境

### WireGuard 隧道 (wireguard.rs)

**功能**：集成WireGuard VPN的隧道实现

**主要特点**：
- 高性能
- 强加密
- 低开销

### QUIC 隧道 (quic.rs)

**功能**：基于QUIC协议的隧道实现

**主要特点**：
- 低延迟
- 多路复用
- 连接迁移
- 内置加密

### 假TCP 隧道 (fake_tcp/)

**功能**：基于UDP的类TCP隧道实现

**主要特点**：
- 避免TCP的头部拥塞控制
- 适用于高延迟网络
- 支持各种平台的网络过滤器

## 核心功能

### 隧道建立

隧道模块支持多种隧道建立方式，包括：
- 直接连接
- 通过中继服务器连接
- 通过打洞技术连接

### 数据包处理

隧道模块负责处理各种类型的数据包，包括：
- 封装和解封装数据包
- 加密和解密数据包
- 处理数据包重传
- 处理数据包分片和重组

### 连接管理

隧道模块负责管理隧道连接的生命周期，包括：
- 连接的建立和关闭
- 连接的心跳检测
- 连接的错误处理
- 连接的重连机制

### 流量统计

隧道模块提供了流量统计功能，包括：
- 发送和接收的数据包数量
- 发送和接收的字节数
- 连接的延迟和丢包率

## 接口说明

### 隧道特质 (Tunnel)

**功能**：定义隧道的基本接口

**主要方法**：
- `send`：发送数据包
- `recv`：接收数据包
- `close`：关闭隧道
- `local_addr`：获取本地地址
- `peer_addr`：获取对等节点地址

### 隧道监听器特质 (TunnelListener)

**功能**：定义隧道监听器的接口

**主要方法**：
- `accept`：接受新的隧道连接
- `local_addr`：获取本地地址
- `close`：关闭监听器

### 隧道连接器特质 (TunnelConnector)

**功能**：定义隧道连接器的接口

**主要方法**：
- `connect`：连接到远程地址

### 主要函数

#### `build_url_from_socket_addr`

**功能**：从套接字地址构建URL

**参数**：
- `addr`：套接字地址
- `scheme`：URL方案

**返回值**：构建的URL

## 使用示例

### 创建TCP隧道

```rust
// 创建TCP隧道监听器
let listener = TcpTunnelListener::bind("0.0.0.0:8080").await?;

// 接受连接
let (tunnel, addr) = listener.accept().await?;

// 发送数据
tunnel.send(&data).await?;

// 接收数据
let data = tunnel.recv().await?;
```

### 创建UDP隧道

```rust
// 创建UDP隧道
let tunnel = UdpTunnel::new("127.0.0.1:8080", "192.168.1.100:8080").await?;

// 发送数据
tunnel.send(&data).await?;

// 接收数据
let data = tunnel.recv().await?;
```

### 创建WebSocket隧道

```rust
// 创建WebSocket隧道
let tunnel = WebSocketTunnel::connect("ws://example.com:8080").await?;

// 发送数据
tunnel.send(&data).await?;

// 接收数据
let data = tunnel.recv().await?;
```

### 创建WireGuard隧道

```rust
// 创建WireGuard隧道
let config = WireGuardConfig {
    private_key: "private_key".to_string(),
    public_key: "public_key".to_string(),
    endpoint: "192.168.1.100:51820".parse()?,
    ..Default::default()
};
let tunnel = WireGuardTunnel::new(config).await?;

// 发送数据
tunnel.send(&data).await?;

// 接收数据
let data = tunnel.recv().await?;
```

## 注意事项

1. **协议选择**：根据网络环境和需求选择合适的隧道协议，例如：
   - 对可靠性要求高的场景使用TCP隧道
   - 对延迟要求低的场景使用UDP或QUIC隧道
   - 需要穿透防火墙的场景使用WebSocket隧道

2. **加密配置**：确保隧道使用适当的加密配置，保护数据传输的安全性。

3. **性能优化**：根据实际情况调整隧道的缓冲区大小、超时设置等参数，优化性能。

4. **错误处理**：实现适当的错误处理机制，处理隧道连接失败、数据包丢失等情况。

5. **资源管理**：确保隧道连接在不需要时及时关闭，避免资源泄漏。

6. **平台兼容性**：注意不同平台的网络特性差异，特别是在使用假TCP隧道时。