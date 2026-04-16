# 核心模块文档

## 概述

exvlan 的核心模块提供了创建和管理虚拟网络的基础功能，包括隧道建立、对等节点通信、代理服务和网络配置。

## 模块结构

核心模块由多个子模块组成，每个子模块负责特定的功能：

- **core**：核心功能，包括命令行解析、内存分析和主入口点
- **tunnel**：各种隧道协议实现（TCP、UDP、WebSocket、WireGuard、QUIC）
- **peers**：对等节点通信，包括连接管理、加密和路由
- **instance**：网络实例管理，包括DNS服务器、监听器和代理CIDR监控
- **gateway**：代理服务实现（TCP、UDP、ICMP、SOCKS5）
- **rpc_service**：用于管理实例、对等节点、连接器等的RPC服务
- **common**：跨模块使用的公共工具和实用程序
- **connector**：各种连接方法（直接连接、DNS、HTTP、打洞）
- **proto**：协议定义和RPC实现
- **peer_center**：对等节点注册和发现
- **web_client**：Web界面客户端实现

## 主要功能

### 隧道协议
- **TCP**：基于TCP的可靠隧道
- **UDP**：基于UDP的快速隧道
- **WebSocket**：基于WebSocket协议的隧道
- **WireGuard**：与WireGuard VPN集成
- **QUIC**：基于现代QUIC协议的隧道
- **Fake TCP**：基于UDP的类TCP隧道

### 代理服务
- **TCP代理**：代理TCP流量
- **UDP代理**：代理UDP流量
- **ICMP代理**：代理ICMP流量
- **SOCKS5代理**：支持SOCKS5协议
- **Fast SOCKS5**：高性能SOCKS5实现

### 对等节点管理
- **对等节点发现**：查找并连接到对等节点
- **加密**：安全的对等节点通信
- **路由**：优化对等节点之间的流量路由
- **中继**：通过中间对等节点中继流量
- **ACL过滤**：对等节点的访问控制

### 实例管理
- **DNS服务器**：虚拟网络的自定义DNS服务器
- **监听器**：管理网络监听器
- **代理CIDR监控**：监控和更新代理CIDR
- **虚拟网卡**：虚拟网络接口管理

### RPC服务
- **实例管理**：创建、启动、停止和配置实例
- **对等节点管理**：添加、删除和管理对等节点
- **连接器管理**：配置和管理连接器
- **端口转发**：设置端口转发规则
- **ACL管理**：管理访问控制列表
- **凭证管理**：管理用于认证的凭证

## 快速开始

### 前提条件
- Rust编程语言
- Tokio异步运行时
- 特定功能的各种依赖项

### 安装

```bash
# 克隆仓库
git clone <repository-url>

# 构建项目
cd exvlan/core
cargo build

# 运行项目
cargo run
```

### 配置

核心模块可以通过命令行参数或配置文件进行配置。关键配置选项包括：

- **网络设置**：IP地址、子网、DNS设置
- **隧道设置**：协议、端口、加密
- **对等节点设置**：发现方法、认证
- **代理设置**：CIDR范围、端口转发

### 使用示例

#### 创建网络实例

```bash
# 创建新的网络实例
cargo run -- create --name my-network --ipv4 10.0.0.1/24

# 启动实例
cargo run -- start --name my-network

# 停止实例
cargo run -- stop --name my-network
```

#### 添加对等节点

```bash
# 添加对等节点
cargo run -- peer add --name my-network --address 192.168.1.100:8080

# 列出对等节点
cargo run -- peer list --name my-network

# 删除对等节点
cargo run -- peer remove --name my-network --peer-id 12345
```

#### 配置端口转发

```bash
# 添加端口转发规则
cargo run -- port-forward add --name my-network --local-port 8080 --remote-addr 10.0.0.2:80

# 列出端口转发规则
cargo run -- port-forward list --name my-network

# 删除端口转发规则
cargo run -- port-forward remove --name my-network --local-port 8080
```

## API文档

核心模块提供了一个全面的RPC API，用于管理网络实例及其组件。API分为多个服务：

- **InstanceService**：管理网络实例
- **PeerService**：管理对等节点
- **ConnectorService**：管理连接器
- **PortForwardService**：管理端口转发
- **ACLService**：管理访问控制列表
- **CredentialService**：管理凭证
- **StatsService**：获取统计信息和指标

## 故障排除

### 常见问题

- **连接失败**：检查网络连接、防火墙设置和对等节点地址
- **DNS解析问题**：验证DNS服务器配置和网络设置
- **性能问题**：检查隧道协议设置、加密配置和网络带宽
- **认证失败**：验证凭证和认证设置

### 日志

核心模块使用结构化日志来帮助诊断问题。可以通过命令行或配置文件配置日志。

## 贡献

欢迎对核心模块做出贡献。请遵循项目的编码标准并提交拉取请求进行审查。

## 许可证

核心模块以 [MIT许可证](LICENSE) 发布。