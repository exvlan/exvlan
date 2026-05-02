# 使用说明

本指南将帮助您了解如何使用 `exvlan-rpc` 客户端库与 era-xvlan 的 RPC 服务进行通信。

## 基本用法

### 创建客户端

首先，您需要创建一个 `RpcClient` 实例，连接到 RPC 服务器：

```rust
use exvlan_rpc::client::RpcClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 创建 RPC 客户端，连接到指定地址
    let client = RpcClient::new("127.0.0.1:50051").await?;
    
    // 使用客户端...
    
    Ok(())
}
```

### 获取服务客户端

`RpcClient` 提供了方法来获取各个服务的客户端：

```rust
use exvlan::common::PeerId;

// 获取对等节点管理客户端
let peer_manage = client.peer_manage(
    PeerId::from(0),  // 源对等节点 ID
    PeerId::from(1),  // 目标对等节点 ID
    "example.com".to_string()  // 域名
);

// 获取配置管理客户端
let config = client.config(
    PeerId::from(0),
    PeerId::from(1),
    "example.com".to_string()
);

// 获取统计信息客户端
let stats = client.stats(
    PeerId::from(0),
    PeerId::from(1),
    "example.com".to_string()
);
```

### 调用服务方法

每个服务客户端都提供了对应的方法来调用 RPC 服务：

```rust
// 调用对等节点管理服务的 list_peer 方法
let peers = peer_manage.list_peer().await?;
println!("Peers: {:?}", peers);

// 调用配置管理服务的 get_config 方法
let config_response = config.get_config().await?;
println!("Config: {:?}", config_response);

// 调用统计信息服务的 get_stats 方法
let stats_response = stats.get_stats().await?;
println!("Stats: {:?}", stats_response);
```

## 服务使用示例

### 1. 对等节点管理 (peer_manage)

```rust
// 列出所有对等节点
let peers = peer_manage.list_peer().await?;

// 列出所有路由
let routes = peer_manage.list_route().await?;

// 导出路由
let dumped_routes = peer_manage.dump_route().await?;
```

### 2. 配置管理 (config)

```rust
// 获取配置
let config = config.get_config().await?;

// 更新配置
let patch_request = exvlan::proto::api::config::PatchConfigRequest {
    // 配置更新内容
    ..Default::default()
};
let patch_response = config.patch_config(patch_request).await?;
```

### 3. VPN 门户 (vpn_portal)

```rust
// 获取 VPN 门户信息
let vpn_info = vpn_portal.get_vpn_portal_info().await?;
```

### 4. 统计信息 (stats)

```rust
// 获取统计信息
let stats = stats.get_stats().await?;

// 获取 Prometheus 格式的统计信息
let prometheus_stats = stats.get_prometheus_stats().await?;
```

### 5. 日志管理 (logger)

```rust
// 获取日志配置
let logger_config = logger.get_logger_config().await?;

// 设置日志配置
let set_config_request = exvlan::proto::api::logger::SetLoggerConfigRequest {
    // 日志配置
    ..Default::default()
};
let set_config_response = logger.set_logger_config(set_config_request).await?;
```

## 错误处理

`exvlan-rpc` 使用 `anyhow::Result` 来处理错误，您可以使用标准的 Rust 错误处理方式：

```rust
use anyhow::Context;

async fn example() -> anyhow::Result<()> {
    let client = RpcClient::new("127.0.0.1:50051").await
        .context("Failed to connect to RPC server")?;
    
    let peer_manage = client.peer_manage(
        PeerId::from(0),
        PeerId::from(1),
        "example.com".to_string()
    );
    
    let peers = peer_manage.list_peer().await
        .context("Failed to list peers")?;
    
    Ok(())
}
```

## 最佳实践

### 1. 重用客户端

创建 `RpcClient` 实例可能会比较耗时，建议在应用程序启动时创建一次，然后在整个应用程序中重用：

```rust
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = Arc::new(RpcClient::new("127.0.0.1:50051").await?);
    
    // 在多个任务中使用同一个客户端
    let client_clone = client.clone();
    tokio::spawn(async move {
        let peer_manage = client_clone.peer_manage(
            PeerId::from(0),
            PeerId::from(1),
            "example.com".to_string()
        );
        // 使用 peer_manage...
    });
    
    Ok(())
}
```

### 2. 处理连接失败

当 RPC 服务器不可用时，客户端连接会失败。建议添加适当的错误处理和重试机制：

```rust
async fn connect_with_retry(addr: &str) -> anyhow::Result<RpcClient> {
    const MAX_RETRIES: usize = 3;
    let mut retries = 0;
    
    loop {
        match RpcClient::new(addr).await {
            Ok(client) => return Ok(client),
            Err(e) => {
                retries += 1;
                if retries >= MAX_RETRIES {
                    return Err(e);
                }
                println!("Connection failed, retrying... ({} of {})");
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }
        }
    }
}
```

### 3. 合理设置超时

对于 RPC 调用，建议设置合理的超时时间，避免长时间阻塞：

```rust
use tokio::time::timeout;
use tokio::time::Duration;

async fn call_with_timeout<T>(future: impl futures::Future<Output = anyhow::Result<T>>) -> anyhow::Result<T> {
    timeout(Duration::from_secs(5), future)
        .await
        .context("RPC call timed out")?
}

// 使用示例
let peers = call_with_timeout(peer_manage.list_peer()).await?;
```

## 示例代码

完整的示例代码可以在 [examples/basic_usage.rs](examples/basic_usage.rs) 中找到。

## 下一步

了解更多 API 详情，请查看 [API 参考](api_reference.md)。
