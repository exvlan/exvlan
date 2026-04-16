# exvlan-rpc

## 项目简介

`exvlan-rpc` 是一个 RPC 客户端库，用于与 era-xvlan 项目的 RPC 服务进行通信。它提供了一组客户端工具，用于调用 era-xvlan 中的各种服务，如对等节点管理、连接器管理、VPN 门户等。

## 功能特性

- 支持所有 era-xvlan 的 RPC 服务
- 异步 API 设计，基于 Tokio
- 统一的错误处理
- 模块化的服务客户端设计
- 支持多种服务类型

## 快速开始

### 安装

在 `Cargo.toml` 中添加依赖：

```toml
dependencies {
    exvlan-rpc = { path = "path/to/exvlan/rpc" }
    tokio = { version = "1.35.1", features = ["full"] }
    anyhow = "1.0.102"
}
```

### 基本使用

```rust
use exvlan::common::PeerId;
use exvlan_rpc::client::RpcClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 创建 RPC 客户端
    let client = RpcClient::new("127.0.0.1:50051").await?;
    
    // 获取对等节点管理客户端
    let peer_manage = client.peer_manage(PeerId::from(0), PeerId::from(1), "example.com".to_string());
    
    // 调用方法
    let peers = peer_manage.list_peer().await?;
    println!("Peers: {:?}", peers);
    
    Ok(())
}
```

## 服务列表

- **peer_manage** - 对等节点管理
- **connector_manage** - 连接器管理
- **mapped_listener_manage** - 映射监听器管理
- **vpn_portal** - VPN 门户
- **tcp_proxy** - TCP 代理
- **acl_manage** - ACL 管理
- **port_forward_manage** - 端口转发管理
- **stats** - 统计信息
- **config** - 配置管理
- **logger** - 日志管理
- **instance_manage** - 实例管理
- **peer_center** - 对等节点中心
- **credential_manage** - 凭证管理

## 文档

- [安装指南](installation.md)
- [使用说明](usage.md)
- [API 参考](api_reference.md)
- [示例代码](examples/basic_usage.rs)

## 测试

运行测试：

```bash
cargo test
```

## 贡献

欢迎贡献代码和文档！
