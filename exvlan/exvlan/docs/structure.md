# 目录结构

## 核心模块结构

```
exvlan/core/src/
├── lib.rs              # 核心库入口
├── core.rs             # 核心功能实现
├── main.rs             # 主入口
├── cli.rs              # 命令行界面
├── launcher.rs         # 启动器
├── utils.rs            # 工具函数
├── arch/               # 架构相关代码
│   ├── mod.rs
│   └── windows.rs
├── common/             # 公共模块
│   ├── mod.rs
│   ├── acl_processor.rs
│   ├── compressor.rs
│   ├── config.rs
│   ├── constants.rs
│   ├── defer.rs
│   ├── dns.rs
│   ├── env_parser.rs
│   ├── error.rs
│   ├── global_ctx.rs
│   ├── idn.rs
│   ├── ifcfg/          # 网络接口配置
│   ├── log.rs
│   ├── netns.rs
│   ├── network.rs
│   ├── os_info.rs
│   ├── scoped_task.rs
│   ├── stats_manager.rs
│   ├── stun.rs
│   ├── stun_codec_ext.rs
│   ├── token_bucket.rs
│   └── tracing_rolling_appender/
├── connector/          # 连接器模块
│   ├── mod.rs
│   ├── direct.rs
│   ├── dns_connector.rs
│   ├── http_connector.rs
│   ├── manual.rs
│   ├── tcp_hole_punch.rs
│   └── udp_hole_punch/  # UDP打洞实现
├── gateway/            # 网关模块
│   ├── mod.rs
│   ├── fast_socks5/     # 快速SOCKS5代理
│   ├── icmp_proxy.rs
│   ├── ip_reassembler.rs
│   ├── socks5.rs
│   ├── tcp_proxy.rs
│   ├── tokio_smoltcp/   # SmolTCP集成
│   ├── udp_proxy.rs
│   └── wrapped_proxy.rs
├── instance/           # 实例管理模块
│   ├── mod.rs
│   ├── dns_server/      # DNS服务器
│   ├── instance.rs
│   ├── listeners.rs
│   ├── proxy_cidrs_monitor.rs
│   └── virtual_nic.rs
├── instance_manager.rs  # 实例管理器
├── peer_center/        # 对等节点中心
│   ├── mod.rs
│   ├── instance.rs
│   └── server.rs
├── peers/              # 对等节点模块
│   ├── mod.rs
│   ├── acl_filter.rs
│   ├── credential_manager.rs
│   ├── encrypt/         # 加密实现
│   ├── foreign_network_client.rs
│   ├── foreign_network_manager.rs
│   ├── graph_algo.rs
│   ├── peer.rs
│   ├── peer_conn.rs
│   ├── peer_conn_ping.rs
│   ├── peer_manager.rs
│   ├── peer_map.rs
│   ├── peer_ospf_route.rs
│   ├── peer_rpc.rs
│   ├── peer_rpc_service.rs
│   ├── peer_session.rs
│   ├── peer_task.rs
│   ├── relay_peer_map.rs
│   ├── rpc_service.rs
│   ├── route_trait.rs
│   └── traffic_metrics.rs
├── proto/              # 协议定义
│   ├── mod.rs
│   ├── api.rs
│   ├── common.rs
│   ├── peer_rpc.proto
│   ├── peer_rpc.rs
│   ├── rpc_impl/        # RPC实现
│   ├── rpc_types/       # RPC类型
│   └── web.rs
├── rpc_service/        # RPC服务模块
│   ├── mod.rs
│   ├── acl_manage.rs
│   ├── api.rs
│   ├── config.rs
│   ├── connector_manage.rs
│   ├── credential_manage.rs
│   ├── instance_manage.rs
│   ├── logger.rs
│   ├── mapped_listener_manage.rs
│   ├── peer_center.rs
│   ├── peer_manage.rs
│   ├── port_forward_manage.rs
│   ├── protected_port.rs
│   ├── proxy.rs
│   ├── remote_client.rs
│   ├── stats.rs
│   └── vpn_portal.rs
├── service_manager/    # 服务管理器
│   └── mod.rs
├── tests/              # 测试文件
│   ├── mod.rs
│   ├── credential_tests.rs
│   ├── ipv6_test.rs
│   └── three_node.rs
├── tunnel/             # 隧道模块
│   ├── mod.rs
│   ├── buf.rs
│   ├── common.rs
│   ├── fake_tcp/        # 假TCP实现
│   ├── filter.rs
│   ├── insecure_tls.rs
│   ├── mpsc.rs
│   ├── packet_def.rs
│   ├── quic.rs
│   ├── ring.rs
│   ├── stats.rs
│   ├── tcp.rs
│   ├── udp.rs
│   ├── unix.rs
│   ├── websocket.rs
│   └── wireguard.rs
└── web_client/         # Web客户端
    ├── mod.rs
    ├── controller.rs
    ├── security.rs
    └── session.rs
```

## 模块说明

### 核心模块 (core)
- **lib.rs**: 核心库的入口点，导出主要模块和功能
- **core.rs**: 核心功能实现，包括内存分析、CLI解析等
- **main.rs**: 程序主入口
- **cli.rs**: 命令行界面实现
- **launcher.rs**: 启动器，负责启动和管理实例
- **utils.rs**: 通用工具函数

### 公共模块 (common)
提供各种公共功能和工具，包括配置管理、日志、网络接口配置、错误处理等。

### 连接器模块 (connector)
实现各种连接方式，包括直接连接、DNS连接、HTTP连接、TCP打洞和UDP打洞等。

### 网关模块 (gateway)
实现各种代理功能，包括TCP代理、UDP代理、ICMP代理、SOCKS5代理等。

### 实例管理模块 (instance)
管理网络实例，包括DNS服务器、监听器、代理CIDR监控等。

### 对等节点模块 (peers)
管理对等节点的连接、会话、路由等功能，包括加密、ACL过滤等。

### 隧道模块 (tunnel)
实现各种隧道协议，包括TCP、UDP、WebSocket、WireGuard、QUIC等。

### RPC服务模块 (rpc_service)
提供各种RPC服务，包括实例管理、对等节点管理、连接器管理、端口转发管理等。

### 协议定义 (proto)
定义各种协议和RPC接口，包括API、对等节点通信等。

### 对等节点中心 (peer_center)
管理对等节点的注册和发现。

### 服务管理器 (service_manager)
管理各种服务的生命周期。

### Web客户端 (web_client)
提供Web界面的客户端实现。

### 架构相关 (arch)
提供特定架构的实现，如Windows平台特定代码。