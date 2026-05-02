# 安装指南

本指南将帮助您在项目中安装和配置 `exvlan-rpc` 客户端库。

## 前提条件

在安装 `exvlan-rpc` 之前，您需要确保：

1. 您的项目使用 Rust 编程语言
2. 您已经安装了 Rust 工具链（`cargo`、`rustc` 等）
3. 您的项目依赖于 Tokio 异步运行时
4. 您已经克隆了 era-xvlan 项目

## 安装步骤

### 步骤 1：添加依赖

在您的项目的 `Cargo.toml` 文件中添加以下依赖：

```toml
[dependencies]
# 核心依赖
exvlan-rpc = { path = "path/to/exvlan/rpc" }  # 替换为实际路径
tokio = { version = "1.35.1", features = ["full"] }  # 异步运行时
anyhow = "1.0.102"  # 错误处理

# 可选依赖
tracing = "0.1.40"  # 日志追踪
```

### 步骤 2：构建项目

运行以下命令构建您的项目：

```bash
cargo build
```

### 步骤 3：验证安装

创建一个简单的测试文件来验证安装是否成功：

```rust
// src/main.rs
use exvlan::common::PeerId;
use exvlan_rpc::client::RpcClient;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("exvlan-rpc 安装成功！");
    
    // 注意：实际使用时需要连接到真实的 RPC 服务器
    // let client = RpcClient::new("127.0.0.1:50051").await?;
    // println!("客户端连接成功！");
    
    Ok(())
}
```

运行测试：

```bash
cargo run
```

如果输出 "exvlan-rpc 安装成功！"，则说明安装成功。

## 依赖说明

### 核心依赖

- **exvlan** - 核心库，提供 RPC 服务定义和 protobuf 类型
- **tokio** - 异步运行时，用于处理异步操作
- **anyhow** - 错误处理库，提供统一的错误类型

### 测试依赖

- **mockall** - 用于模拟测试
- **assert_matches** - 用于断言匹配

## 常见问题

### 找不到 exvlan 依赖

确保您已经正确设置了 `exvlan` 的路径。如果 `exvlan-rpc` 和 `exvlan` 在同一个工作区中，您可以使用相对路径：

```toml
exvlan-rpc = { path = "../rpc" }
```

### 编译错误

如果遇到编译错误，请检查：

1. Rust 版本是否兼容（推荐 Rust 1.60+）
2. 依赖版本是否匹配
3. 路径设置是否正确

### 连接失败

如果客户端连接失败，请检查：

1. RPC 服务器是否正在运行
2. 服务器地址和端口是否正确
3. 网络连接是否正常

## 版本兼容性

| exvlan-rpc 版本 | exvlan 版本 | Rust 版本 |
|----------------|-------------|-----------|
| 0.1.0          | 0.1.0       | 1.60+     |

## 下一步

安装完成后，您可以查看 [使用说明](usage.md) 了解如何使用 `exvlan-rpc` 客户端库。
