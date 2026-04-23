use exvlan::common::PeerId;
use exvlan_rpc::client::RpcClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== exvlan-rpc 基本使用示例 ===");
    
    // 步骤 1: 创建 RPC 客户端
    println!("1. 创建 RPC 客户端...");
    // 注意：实际使用时需要连接到真实的 RPC 服务器
    // let client = RpcClient::new("127.0.0.1:50051").await?;
    // println!("客户端连接成功！");
    println!("（注：这里只是示例，实际使用时需要连接到真实的 RPC 服务器）");
    
    // 步骤 2: 设置参数
    let from_peer_id = PeerId::from(0);
    let to_peer_id = PeerId::from(1);
    let domain_name = "example.com".to_string();
    
    println!("2. 设置参数:");
    println!("   - 源对等节点 ID: {}", from_peer_id);
    println!("   - 目标对等节点 ID: {}", to_peer_id);
    println!("   - 域名: {}", domain_name);
    
    // 步骤 3: 获取服务客户端
    println!("\n3. 获取服务客户端...");
    // 注意：实际使用时需要从 client 实例获取
    /*
    let peer_manage = client.peer_manage(from_peer_id, to_peer_id, domain_name.clone());
    let config = client.config(from_peer_id, to_peer_id, domain_name.clone());
    let stats = client.stats(from_peer_id, to_peer_id, domain_name);
    */
    println!("   - 对等节点管理客户端");
    println!("   - 配置管理客户端");
    println!("   - 统计信息客户端");
    
    // 步骤 4: 调用服务方法
    println!("\n4. 调用服务方法...");
    // 注意：实际使用时需要调用真实的方法
    /*
    // 调用对等节点管理服务
    let peers = peer_manage.list_peer().await?;
    println!("   对等节点列表: {:?}", peers);
    
    // 调用配置管理服务
    let config_response = config.get_config().await?;
    println!("   配置信息: {:?}", config_response);
    
    // 调用统计信息服务
    let stats_response = stats.get_stats().await?;
    println!("   统计信息: {:?}", stats_response);
    */
    println!("   - 调用 list_peer() 获取对等节点列表");
    println!("   - 调用 get_config() 获取配置信息");
    println!("   - 调用 get_stats() 获取统计信息");
    
    // 步骤 5: 错误处理示例
    println!("\n5. 错误处理示例:");
    println!("   使用 anyhow::Result 进行错误处理:");
    println!("   ");
    println!("   async fn example() -> anyhow::Result<()> {{");
    println!("       let client = RpcClient::new(\"127.0.0.1:50051\").await?;");
    println!("       let peer_manage = client.peer_manage(from_peer_id, to_peer_id, domain_name.clone());");
    println!("       let peers = peer_manage.list_peer().await?;");
    println!("       Ok(())");
    println!("   }}");
    
    // 步骤 6: 最佳实践
    println!("\n6. 最佳实践:");
    println!("   - 重用客户端实例");
    println!("   - 添加适当的错误处理");
    println!("   - 设置合理的超时时间");
    println!("   - 处理连接失败的情况");
    
    println!("\n=== 示例结束 ===");
    println!("请根据实际情况修改代码，连接到真实的 RPC 服务器。");
    
    Ok(())
}

/*
完整的实际使用示例：

use exvlan::common::PeerId;
use exvlan_rpc::client::RpcClient;
use anyhow::Context;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 创建客户端
    let client = RpcClient::new("127.0.0.1:50051").await
        .context("Failed to connect to RPC server")?;
    
    // 设置参数
    let from_peer_id = PeerId::from(0);
    let to_peer_id = PeerId::from(1);
    let domain_name = "example.com".to_string();
    
    // 获取对等节点管理客户端
    let peer_manage = client.peer_manage(from_peer_id, to_peer_id, domain_name.clone());
    
    // 调用方法
    let peers = peer_manage.list_peer().await
        .context("Failed to list peers")?;
    println!("Peers: {:?}", peers);
    
    // 获取配置管理客户端
    let config = client.config(from_peer_id, to_peer_id, domain_name.clone());
    
    // 调用方法
    let config_response = config.get_config().await
        .context("Failed to get config")?;
    println!("Config: {:?}", config_response);
    
    // 获取统计信息客户端
    let stats = client.stats(from_peer_id, to_peer_id, domain_name);
    
    // 调用方法
    let stats_response = stats.get_stats().await
        .context("Failed to get stats")?;
    println!("Stats: {:?}", stats_response);
    
    Ok(())
}
*/
