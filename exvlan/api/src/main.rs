#![allow(dead_code)]

#[macro_use]
extern crate rust_i18n;

use std::net::IpAddr;
use std::sync::Arc;

use central_network::gateway;

use clap::Parser;
use exvlan::common::log as exvlan_rpc_log;
use exvlan::tunnel::websocket::WsTunnelListener;
use exvlan::{
    common::{
        config::{ConsoleLoggerConfig, FileLoggerConfig, LoggingConfigLoader},
        constants::EXVLAN_VERSION,
        error::Error,
        log,
        network::{local_ipv4, local_ipv6},
    },
    proto::rpc::standalone::{runtime_rpc_listener, runtime_udp_tunnel_listener},
    utils::panic::setup_panic_handler,
};
use exvlan_core::{socket::SocketListener, tunnel::Tunnel};

use exvlan::tunnel::IpScheme;
use mimalloc::MiMalloc;

mod central_network;
mod client_manager;
mod db;
mod migrator;
mod restful;
mod webhook;

#[cfg(feature = "embed")]
mod web;

#[global_allocator]
static GLOBAL_MIMALLOC: MiMalloc = MiMalloc;

rust_i18n::i18n!("locales", fallback = "en");

#[derive(Parser, Debug)]
#[command(name = "exvlan-api", author, version = EXVLAN_VERSION , about, long_about = None)]
struct Cli {
    #[arg(
        short,
        long,
        env = "ET_WEB_DB",
        default_value = "et.db",
        help = t!("cli.db").to_string()
    )]
    db: String,

    #[arg(
        long,
        env = "ET_WEB_CONSOLE_LOG_LEVEL",
        help = t!("cli.console_log_level").to_string(),
    )]
    console_log_level: Option<String>,

    #[arg(
        long,
        env = "ET_WEB_FILE_LOG_LEVEL",
        help = t!("cli.file_log_level").to_string(),
    )]
    file_log_level: Option<String>,

    #[arg(
        long,
        env = "ET_WEB_FILE_LOG_DIR",
        help = t!("cli.file_log_dir").to_string(),
    )]
    file_log_dir: Option<String>,

    /// config-server 监听地址（可选）。默认 `0.0.0.0`；v6 仍额外监听 `[::]`。
    /// 只影响 config-server（默认 22020），不影响 REST API（11211）。
    #[arg(
        long = "host",
        env = "ET_HOST",
        default_value = "0.0.0.0",
        help = "config-server listen address (default 0.0.0.0)"
    )]
    host: IpAddr,

    #[arg(
        long = "port",
        short = 'c',
        alias = "config-server-port",
        env = "ET_CONFIG_SERVER_PORT",
        default_value = "22020",
        help = t!("cli.config_server_port").to_string(),
    )]
    port: u16,

    /// 可逗号分隔多个协议（`udp,ws`）；每个协议各起一组监听。
    #[arg(
        long = "protocol",
        short = 'p',
        alias = "config-server-protocol",
        env = "ET_CONFIG_SERVER_PROTOCOL",
        default_value = "udp,ws",
        value_delimiter = ',',
        help = t!("cli.config_server_protocol").to_string(),
    )]
    protocol: Vec<String>,

    #[arg(
        long,
        short='a',
        env = "ET_API_SERVER_PORT",
        default_value = "11211",
        help = t!("cli.api_server_port").to_string(),
    )]
    api_server_port: u16,

    #[arg(
        long,
        env = "ET_API_SERVER_ADDR",
        default_value = "0.0.0.0",
        help = t!("cli.api_server_addr").to_string(),
    )]
    api_server_addr: IpAddr,

    #[arg(
        long,
        env = "ET_GEOIP_DB",
        help = t!("cli.geoip_db").to_string(),
    )]
    geoip_db: Option<String>,

    #[arg(
        long,
        env = "ET_HEARTBEAT_MIN_RESPONSE_MS",
        default_value = "3500",
        help = t!("cli.heartbeat_min_response_ms").to_string(),
    )]
    heartbeat_min_response_ms: u64,

    #[arg(
        long,
        env = "ET_HEARTBEAT_TIMEOUT_MS",
        default_value = "15000",
        help = t!("cli.heartbeat_timeout_ms").to_string(),
    )]
    heartbeat_timeout_ms: u64,

    #[arg(long, env = "ET_GATEWAY_PEER_URL")]
    gateway_peer_url: Option<url::Url>,

    #[arg(long, env = "ET_GATEWAY_RELAY_DATA", default_value = "false")]
    gateway_relay_data: bool,

    #[cfg(feature = "embed")]
    #[arg(
        long,
        short='l',
        env = "ET_WEB_SERVER_PORT",
        help = t!("cli.web_server_port").to_string(),
    )]
    web_server_port: Option<u16>,

    #[cfg(feature = "embed")]
    #[arg(
        long,
        env = "ET_WEB_SERVER_ADDR",
        default_value = "0.0.0.0",
        help = t!("cli.web_server_addr").to_string(),
    )]
    web_server_addr: IpAddr,

    #[cfg(feature = "embed")]
    #[arg(
        long,
        env = "ET_NO_WEB",
        help = t!("cli.no_web").to_string(),
        default_value = "false"
    )]
    no_web: bool,

    #[cfg(feature = "embed")]
    #[arg(
        long,
        env = "ET_API_HOST",
        help = t!("cli.api_host").to_string()
    )]
    api_host: Option<url::Url>,

    #[command(flatten)]
    feature_flags: FeatureFlags,

    #[command(flatten)]
    oidc: restful::oidc::OidcOptions,

    #[command(flatten)]
    webhook: WebhookOptions,
}

#[derive(Debug, Clone, Default, clap::Args)]
pub struct WebhookOptions {
    /// Base URL of the webhook endpoint for token validation and event delivery.
    /// When set, incoming tokens are validated via this webhook before local fallback.
    #[arg(long, env = "ET_WEBHOOK_URL")]
    pub webhook_url: Option<String>,

    /// Shared secret used to authenticate outbound webhook calls.
    #[arg(long, env = "ET_WEBHOOK_SECRET", hide_env_values = true)]
    pub webhook_secret: Option<String>,

    /// Token for X-Internal-Auth header. When set, API requests with this header
    /// bypass session authentication.
    #[arg(long, env = "ET_INTERNAL_AUTH_TOKEN", hide_env_values = true)]
    pub internal_auth_token: Option<String>,

    /// Stable identifier for this exvlan-api instance when routing webhook callbacks.
    #[arg(long, env = "ET_WEB_INSTANCE_ID")]
    pub web_instance_id: Option<String>,

    /// Reachable base URL for this exvlan-api instance's internal REST API.
    #[arg(long, env = "ET_WEB_INSTANCE_API_BASE_URL")]
    pub web_instance_api_base_url: Option<String>,
}

#[derive(Debug, Clone, Default, clap::Args)]
pub struct FeatureFlags {
    /// Whether user registration via the web UI is disabled.
    #[arg(
        long,
        env = "ET_DISABLE_REGISTRATION",
        default_value = "false",
        help = t!("cli.disable_registration").to_string()
    )]
    pub disable_registration: bool,

    /// Whether to auto-create users when they connect via heartbeat with an unknown token.
    #[arg(
        long,
        env = "ET_ALLOW_AUTO_CREATE_USER",
        default_value = "false",
        help = t!("cli.allow_auto_create_user").to_string()
    )]
    pub allow_auto_create_user: bool,
}

impl LoggingConfigLoader for &Cli {
    fn get_console_logger_config(&self) -> ConsoleLoggerConfig {
        ConsoleLoggerConfig {
            level: self.console_log_level.clone(),
        }
    }

    fn get_file_logger_config(&self) -> FileLoggerConfig {
        FileLoggerConfig {
            dir: self.file_log_dir.clone(),
            level: self.file_log_level.clone(),
            file: None,
            size_mb: None,
            count: None,
        }
    }
}

// 日志栈在 `exvlan-rpc`（P7），API 进程同样需要 rpc 侧配置适配。
impl exvlan_rpc_log::LoggingConfigLoader for Cli {
    fn get_console_logger_config(&self) -> exvlan_rpc_log::ConsoleLoggerConfig {
        exvlan_rpc_log::ConsoleLoggerConfig {
            level: self.console_log_level.clone(),
        }
    }

    fn get_file_logger_config(&self) -> exvlan_rpc_log::FileLoggerConfig {
        exvlan_rpc_log::FileLoggerConfig {
            dir: self.file_log_dir.clone(),
            level: self.file_log_level.clone(),
            file: None,
            size_mb: None,
            count: None,
        }
    }
}

pub fn get_listener_by_url(
    scheme: IpScheme,
    l: &url::Url,
) -> Option<Box<dyn SocketListener<Accepted = Box<dyn Tunnel>>>> {
    Some(match scheme {
        IpScheme::Tcp => {
            let addr = l.socket_addrs(|| Some(11010)).ok()?.into_iter().next()?;
            Box::new(runtime_rpc_listener(addr))
        }
        IpScheme::Udp => {
            let addr = l.socket_addrs(|| Some(11010)).ok()?.into_iter().next()?;
            Box::new(runtime_udp_tunnel_listener(l.clone(), addr))
        }
        IpScheme::Ws => Box::new(WsTunnelListener::new(l.clone())),
        _ => return None,
    })
}

async fn get_dual_stack_listener(
    protocol: &str,
    host: IpAddr,
    port: u16,
) -> Result<
    (
        Option<Box<dyn SocketListener<Accepted = Box<dyn Tunnel>>>>,
        Option<Box<dyn SocketListener<Accepted = Box<dyn Tunnel>>>>,
    ),
    Error,
> {
    let scheme = protocol
        .parse()
        .map_err(|_| Error::InvalidUrl(protocol.to_string()))?;
    // v4 绑定地址可配置：多网卡机器上必须指定，否则 UDP 回包会从默认路由的网卡出去，
    // 与入口 EIP 不一致 → 被 NAT 判为不对称丢弃。
    let v4_bind = if host.is_ipv4() {
        host.to_string()
    } else {
        "0.0.0.0".to_string()
    };
    let v6_listener = if local_ipv6().await.is_ok()
        && matches!(scheme, IpScheme::Tcp | IpScheme::Udp | IpScheme::Ws)
    {
        let bind = if host.is_ipv6() {
            host.to_string()
        } else {
            "[::]".to_string()
        };
        get_listener_by_url(
            scheme,
            &format!("{protocol}://{bind}:{port}").parse().unwrap(),
        )
    } else {
        None
    };
    let v4_listener = if local_ipv4().await.is_ok() {
        get_listener_by_url(
            scheme,
            &format!("{protocol}://{v4_bind}:{port}").parse().unwrap(),
        )
    } else {
        None
    };
    Ok((v6_listener, v4_listener))
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() {
    let locale = sys_locale::get_locale().unwrap_or_else(|| String::from("en-US"));
    rust_i18n::set_locale(&locale);
    setup_panic_handler();

    let cli = Cli::parse();
    log::init_with_default_console_targets(&cli, false, &["CORE", "exvlan_web"]).unwrap();
    tracing::info!(
        version = EXVLAN_VERSION,
        web_instance_id = ?cli.webhook.web_instance_id,
        api_address = %cli.api_server_addr,
        api_port = cli.api_server_port,
        config_protocol = %cli.protocol.join(","),
        config_host = %cli.host,
        config_port = cli.port,
        heartbeat_min_response_ms = cli.heartbeat_min_response_ms,
        heartbeat_timeout_ms = cli.heartbeat_timeout_ms,
        webhook_enabled = cli.webhook.webhook_url.as_deref().is_some_and(|url| !url.trim().is_empty()),
        rust_log_override = std::env::var_os("RUST_LOG").is_some(),
        console_log_override = cli.console_log_level.is_some(),
        "exvlan-api starting"
    );

    // Validate OIDC configuration: check split-deploy specific requirements
    // Basic OIDC parameter validation is handled in OidcConfig::from_params
    if cli.oidc.any_param_provided() {
        let is_split_deploy = {
            #[cfg(feature = "embed")]
            {
                let embed_split_by_port = cli.web_server_port.is_some()
                    && cli.web_server_port != Some(cli.api_server_port);
                cli.no_web || embed_split_by_port
            }
            #[cfg(not(feature = "embed"))]
            {
                true
            }
        };

        if is_split_deploy && cli.oidc.oidc_frontend_base_url.is_none() {
            eprintln!("Error: --oidc-frontend-base-url is required in split-deploy mode");
            eprintln!(
                "When frontend and API are deployed separately, you must specify the frontend URL"
            );
            eprintln!("Example: --oidc-frontend-base-url http://your-frontend-domain.com");
            std::process::exit(1);
        }
    }

    // let db = db::Db::new(":memory:").await.unwrap();
    let db = db::Db::new(cli.db).await.unwrap();
    let feature_flags = Arc::new(cli.feature_flags);
    let mut webhook_config = webhook::WebhookConfig::new(
        cli.webhook.webhook_url,
        cli.webhook.webhook_secret,
        cli.webhook.internal_auth_token,
        cli.webhook.web_instance_id,
        cli.webhook.web_instance_api_base_url,
    );
    let central_mode = !webhook_config.has_external_endpoint();
    if central_mode {
        webhook_config =
            webhook_config.with_handler(Arc::new(central_network::device_auth::DeviceAuth::new(
                db.clone(),
                feature_flags.allow_auto_create_user,
            )));
    } else if cli.gateway_peer_url.is_some() {
        eprintln!("Gateway requires central network mode; remove --webhook-url");
        std::process::exit(2);
    }
    let webhook_config = Arc::new(webhook_config);
    let heartbeat_policy = client_manager::HeartbeatPolicy::from_millis(
        cli.heartbeat_min_response_ms,
        cli.heartbeat_timeout_ms,
    )
    .unwrap_or_else(|error| {
        eprintln!("Invalid heartbeat configuration: {error}");
        std::process::exit(2);
    });

    let network_instances = cli.gateway_peer_url.as_ref().map(|peer_url| {
        let config = gateway::GatewayConfig {
            peer_url: peer_url.to_string(),
            relay_data: cli.gateway_relay_data,
        };
        config
            .validate(&cli.protocol.join(","))
            .unwrap_or_else(|error| {
                eprintln!("Invalid Gateway configuration: {error}");
                std::process::exit(2);
            });
        Arc::new(gateway::NetworkInstanceManager::new(config))
    });

    let mut mgr = client_manager::ClientManager::new(
        db.clone(),
        cli.geoip_db,
        heartbeat_policy,
        feature_flags.clone(),
        webhook_config.clone(),
    );
    // 支持一次监听多种协议（默认 udp,ws）：每个协议各起一组 v4/v6 监听。
    let mut config_listeners = Vec::new();
    for protocol in &cli.protocol {
        match get_dual_stack_listener(protocol, cli.host, cli.port).await {
            Ok((v6, v4)) => {
                if v6.is_none() && v4.is_none() {
                    tracing::error!(%protocol, "listen failed on both IPv4 and IPv6");
                }
                config_listeners.extend([v6, v4].into_iter().flatten());
            }
            Err(err) => tracing::error!(%protocol, %err, "create config-server listener failed"),
        }
    }
    if config_listeners.is_empty() {
        panic!("Listen to both IPv4 and IPv6 failed");
    }
    for listener in config_listeners {
        if let Some(instances) = network_instances.as_ref() {
            mgr.add_listener(gateway::listener::GatewayListener::new(
                listener,
                instances.clone(),
            ))
            .await
            .unwrap();
        } else {
            mgr.add_listener(listener).await.unwrap();
        }
    }

    let mgr = Arc::new(mgr);
    let central_service = Arc::new(
        central_network::service::CentralNetworkService::with_gateway(
            db.clone(),
            mgr.clone(),
            network_instances,
        ),
    );
    if central_mode {
        central_service.start_reconciler();
    }

    #[cfg(feature = "embed")]
    let (web_router_restful, web_router_static) = if cli.no_web {
        (None, None)
    } else {
        let web_router = web::build_router(cli.api_host.clone());
        if cli.web_server_port.is_none()
            || (cli.web_server_port == Some(cli.api_server_port)
                && cli.web_server_addr == cli.api_server_addr)
        {
            (Some(web_router), None)
        } else {
            (None, Some(web_router))
        }
    };
    #[cfg(not(feature = "embed"))]
    let web_router_restful = None;

    let oidc_config = if cli.oidc.oidc_issuer_url.is_some() {
        match restful::oidc::OidcConfig::from_params(cli.oidc).await {
            Ok(config) => config,
            Err(e) => {
                eprintln!("Failed to initialize OIDC: {:?}", e);
                eprintln!("Please check your OIDC configuration (issuer URL, client ID, etc.)");
                std::process::exit(1);
            }
        }
    } else {
        restful::oidc::OidcConfig::disabled()
    };

    let _restful_server_tasks = restful::RestfulServer::new(
        std::net::SocketAddr::new(cli.api_server_addr, cli.api_server_port),
        mgr.clone(),
        central_service,
        db,
        web_router_restful,
        feature_flags,
        oidc_config,
        webhook_config,
        // 控制台展示用：取第一个协议（wss 入口在反代侧，不在内核监听里）
        cli.protocol
            .first()
            .cloned()
            .unwrap_or_else(|| "udp".to_string()),
        cli.port,
    )
    .await
    .unwrap()
    .start()
    .await
    .unwrap();

    #[cfg(feature = "embed")]
    let _web_server_task = if let Some(web_router) = web_router_static {
        Some(
            web::WebServer::new(
                std::net::SocketAddr::new(cli.web_server_addr, cli.web_server_port.unwrap_or(0)),
                web_router,
            )
            .await
            .unwrap()
            .start()
            .await
            .unwrap(),
        )
    } else {
        None
    };

    tokio::signal::ctrl_c().await.unwrap();
}
