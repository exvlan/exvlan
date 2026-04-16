# 公共模块

## 模块概述

公共模块是 exvlan 的核心组件之一，提供了各种公共功能和工具，被其他模块广泛使用。该模块包含了配置管理、日志、网络接口配置、错误处理、统计信息等功能，是系统的基础支持组件。

## 目录结构

```
exvlan/core/src/common/
├── mod.rs              # 公共模块入口
├── acl_processor.rs    # ACL处理器
├── compressor.rs       # 压缩器
├── config.rs           # 配置管理
├── constants.rs        # 常量定义
├── defer.rs            # 延迟执行
├── dns.rs              # DNS处理
├── env_parser.rs       # 环境解析
├── error.rs            # 错误处理
├── global_ctx.rs       # 全局上下文
├── idn.rs              # IDN处理
├── ifcfg/              # 网络接口配置
│   ├── mod.rs
│   ├── darwin.rs
│   ├── netlink.rs
│   ├── route.rs
│   └── win/            # Windows特定实现
├── log.rs              # 日志管理
├── netns.rs            # 网络命名空间
├── network.rs          # 网络工具
├── os_info.rs          # 操作系统信息
├── scoped_task.rs      # 作用域任务
├── stats_manager.rs    # 统计信息管理器
├── stun.rs             # STUN协议
├── stun_codec_ext.rs   # STUN编解码器扩展
├── token_bucket.rs     # 令牌桶
└── tracing_rolling_appender/ # 跟踪滚动追加器
    ├── mod.rs
    └── base.rs
```

## 主要组件

### 公共模块入口 (mod.rs)

**功能**：公共模块的入口点，导出主要的公共功能和工具

**主要导出**：
- 各种工具函数
- 全局变量管理
- 日志配置
- 机器ID管理

### 配置管理 (config.rs)

**功能**：管理系统配置

**主要功能**：
- 加载和保存配置
- 验证配置
- 提供配置访问接口
- 处理配置变更

### 日志管理 (log.rs)

**功能**：管理系统日志

**主要功能**：
- 配置日志级别
- 格式化日志输出
- 提供日志宏
- 支持不同的日志输出目标

### 错误处理 (error.rs)

**功能**：提供统一的错误处理机制

**主要功能**：
- 定义错误类型
- 提供错误转换
- 支持错误链
- 提供错误处理工具

### 全局上下文 (global_ctx.rs)

**功能**：提供全局上下文

**主要功能**：
- 管理全局配置
- 提供全局资源访问
- 管理系统状态
- 提供全局服务访问

### 网络接口配置 (ifcfg/)

**功能**：管理网络接口配置

**主要功能**：
- 获取网络接口信息
- 配置网络接口
- 管理路由表
- 适配不同系统的网络配置

### 统计信息管理器 (stats_manager.rs)

**功能**：管理系统统计信息

**主要功能**：
- 收集统计数据
- 提供统计信息查询
- 支持不同类型的统计指标
- 管理统计信息的生命周期

### 令牌桶 (token_bucket.rs)

**功能**：实现令牌桶算法，用于流量控制

**主要功能**：
- 令牌桶实现
- 流量速率控制
- 突发流量处理

### STUN协议 (stun.rs)

**功能**：实现STUN协议，用于NAT穿透

**主要功能**：
- STUN消息处理
- NAT类型检测
- 公共IP地址获取

### 压缩器 (compressor.rs)

**功能**：提供数据压缩功能

**主要功能**：
- 数据压缩
- 数据解压缩
- 支持不同的压缩算法

### 环境解析 (env_parser.rs)

**功能**：解析环境变量

**主要功能**：
- 环境变量解析
- 配置加载
- 环境检测

### 操作系统信息 (os_info.rs)

**功能**：提供操作系统信息

**主要功能**：
- 检测操作系统类型
- 获取操作系统版本
- 提供平台特定信息

### 作用域任务 (scoped_task.rs)

**功能**：提供作用域任务管理

**主要功能**：
- 任务生命周期管理
- 作用域控制
- 资源清理

### 网络命名空间 (netns.rs)

**功能**：管理网络命名空间

**主要功能**：
- 创建和管理网络命名空间
- 网络命名空间切换
- 网络命名空间资源管理

### DNS处理 (dns.rs)

**功能**：提供DNS处理功能

**主要功能**：
- DNS解析
- DNS服务器配置
- DNS缓存

### IDN处理 (idn.rs)

**功能**：提供国际化域名处理

**主要功能**：
- IDN编码
- IDN解码
- 域名验证

### 延迟执行 (defer.rs)

**功能**：提供延迟执行功能

**主要功能**：
- 延迟执行操作
- 资源清理
- 作用域结束时执行

### 常量定义 (constants.rs)

**功能**：定义系统常量

**主要常量**：
- 网络相关常量
- 配置相关常量
- 协议相关常量

### 跟踪滚动追加器 (tracing_rolling_appender/)

**功能**：提供滚动日志追加器

**主要功能**：
- 日志文件滚动
- 日志文件管理
- 日志文件压缩

## 核心功能

### 配置管理

公共模块提供了完整的配置管理功能：
- 支持多种配置格式
- 配置验证和错误处理
- 配置热重载
- 配置默认值

### 日志管理

公共模块提供了完整的日志管理功能：
- 支持不同的日志级别
- 结构化日志输出
- 日志文件管理
- 日志轮转

### 错误处理

公共模块提供了统一的错误处理机制：
- 类型安全的错误处理
- 错误链和上下文
- 错误转换和包装
- 错误显示和格式化

### 全局上下文

公共模块提供了全局上下文管理：
- 全局配置访问
- 全局服务访问
- 全局状态管理
- 资源共享

### 网络工具

公共模块提供了各种网络工具：
- 网络接口配置
- 路由管理
- DNS处理
- STUN协议
- 网络命名空间

### 统计信息

公共模块提供了完整的统计信息功能：
- 计数器
- 仪表盘
- 直方图
- 统计信息查询

### 工具函数

公共模块提供了各种工具函数：
- 字符串处理
- 时间处理
- 随机数生成
- 文件操作
- 系统信息

## 接口说明

### 配置管理 (Config)

**功能**：管理系统配置

**主要方法**：
- `load`：加载配置
- `save`：保存配置
- `get`：获取配置值
- `set`：设置配置值
- `validate`：验证配置

### 日志管理 (Log)

**功能**：管理系统日志

**主要方法**：
- `init`：初始化日志
- `set_level`：设置日志级别
- `info`：信息日志
- `warn`：警告日志
- `error`：错误日志
- `debug`：调试日志

### 全局上下文 (GlobalCtx)

**功能**：提供全局上下文

**主要方法**：
- `new`：创建全局上下文
- `config`：获取配置
- `stats_manager`：获取统计信息管理器
- `net_ns`：获取网络命名空间
- `get_ipv4`：获取IPv4地址

### 统计信息管理器 (StatsManager)

**功能**：管理系统统计信息

**主要方法**：
- `new`：创建统计信息管理器
- `get_counter`：获取计数器
- `get_gauge`：获取仪表盘
- `get_histogram`：获取直方图
- `get_metrics`：获取所有指标

### 令牌桶 (TokenBucket)

**功能**：实现令牌桶算法

**主要方法**：
- `new`：创建令牌桶
- `consume`：消费令牌
- `try_consume`：尝试消费令牌
- `add_tokens`：添加令牌

### STUN客户端 (StunClient)

**功能**：实现STUN客户端

**主要方法**：
- `new`：创建STUN客户端
- `discover`：发现NAT类型和公共IP
- `get_public_addr`：获取公共地址

### 网络接口配置 (IfCfg)

**功能**：管理网络接口配置

**主要方法**：
- `get_interfaces`：获取所有网络接口
- `get_interface`：获取指定网络接口
- `add_route`：添加路由
- `remove_route`：移除路由

## 使用示例

### 配置管理

```rust
// 加载配置
let config = Config::load("config.toml").await?;

// 获取配置值
let port = config.get("server.port").unwrap_or(8080);

// 设置配置值
config.set("server.port", 8081).await?;

// 保存配置
config.save("config.toml").await?;
```

### 日志管理

```rust
// 初始化日志
log::init("info").await?;

// 记录信息日志
log::info!("Server started on port {}", port);

// 记录警告日志
log::warn!("Configuration file not found, using defaults");

// 记录错误日志
log::error!("Failed to start server: {:?}", error);

// 记录调试日志
log::debug!("Request received: {:?}", request);
```

### 错误处理

```rust
// 定义错误类型
#[derive(Debug, thiserror::Error)]
enum MyError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Config error: {0}")]
    Config(String),
}

// 使用错误处理
fn process_file() -> Result<(), MyError> {
    let file = std::fs::File::open("config.toml")?;
    // 处理文件
    Ok(())
}
```

### 统计信息

```rust
// 创建统计信息管理器
let stats_manager = StatsManager::new();

// 获取计数器
let counter = stats_manager.get_counter("requests", LabelSet::new());

// 增加计数器
counter.inc();

// 获取仪表盘
let gauge = stats_manager.get_gauge("connections", LabelSet::new());

// 设置仪表盘值
gauge.set(100);

// 获取直方图
let histogram = stats_manager.get_histogram("latency", LabelSet::new());

// 记录直方图值
histogram.observe(10.5);
```

### 令牌桶

```rust
// 创建令牌桶，速率为10个令牌/秒，容量为20
let token_bucket = TokenBucket::new(10.0, 20);

// 消费令牌
if token_bucket.consume(1) {
    // 处理请求
} else {
    // 拒绝请求
}

// 尝试消费令牌
match token_bucket.try_consume(1) {
    Ok(()) => {
        // 处理请求
    }
    Err(_) => {
        // 拒绝请求
    }
}
```

## 注意事项

1. **性能**：公共模块的性能对整个系统的影响很大，需要注意优化：
   - 合理使用缓存
   - 避免不必要的计算
   - 优化内存使用

2. **错误处理**：确保公共模块的错误处理机制合理：
   - 提供清晰的错误信息
   - 支持错误链
   - 便于调试和排查

3. **兼容性**：确保公共模块在不同系统上都能正常工作：
   - 适配不同操作系统
   - 处理系统差异
   - 提供平台特定实现

4. **安全性**：确保公共模块的安全性：
   - 避免安全漏洞
   - 正确处理敏感数据
   - 实现适当的访问控制

5. **可测试性**：确保公共模块易于测试：
   - 提供测试接口
   - 支持模拟和注入
   - 实现单元测试

6. **文档**：确保公共模块有良好的文档：
   - 详细的API文档
   - 使用示例
   - 注意事项和最佳实践