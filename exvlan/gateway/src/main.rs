//! EXVlan 控制面网关（HTTP/JSON + WebSocket）。
//!
//! 定位：**薄**。它是 GUI / CLI / agent / （将来的）Web UI 的共同入口，
//! 只做「鉴权 + 路由 + 转发」，业务判断全部在被调方：
//!
//! - 服务相关（policy / kernel / 安装）→ `exvlan-service`（IPC）
//! - 体检 / 诊断 → `exvlan-service-client::doctor`（同一份 findings）
//!
//! 默认只监听 `127.0.0.1`，鉴权用 Bearer token（`<user>/gateway-token`，0600）。
//! 远端访问需要显式开启 mesh 监听（opt-in），属后续版本。

mod api;

use std::net::SocketAddr;

use anyhow::Context as _;
use axum::Router;
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: String,
    pub port: u16,
    pub token_file: std::path::PathBuf,
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "exvlan_gateway=info,tower_http=warn".into()),
        )
        .init();

    let user = exvlan_service_client::paths::user_paths();
    let token_file = std::env::var("EXVLAN_GATEWAY_TOKEN_FILE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| user.data.join("gateway-token"));

    let cfg = Config {
        bind: env_or("EXVLAN_GATEWAY_BIND", "127.0.0.1"),
        port: env_or(
            "EXVLAN_GATEWAY_PORT",
            &exvlan_service_client::DEFAULT_GATEWAY_PORT.to_string(),
        )
        .parse()
        .context("EXVLAN_GATEWAY_PORT 必须是端口号")?,
        token_file,
    };

    let token = load_or_create_token(&cfg.token_file)?;
    tracing::info!(
        "gateway listening on {}:{} (token file: {})",
        cfg.bind,
        cfg.port,
        cfg.token_file.display()
    );

    let app: Router = api::router(cfg.clone(), token);
    let addr: SocketAddr = format!("{}:{}", cfg.bind, cfg.port)
        .parse()
        .context("非法监听地址")?;
    let listener = TcpListener::bind(addr).await.context("绑定端口失败")?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("gateway 退出")?;
    Ok(())
}

/// 读或生成 token（0600）。
fn load_or_create_token(path: &std::path::Path) -> anyhow::Result<String> {
    if let Ok(text) = std::fs::read_to_string(path) {
        let token = text.trim().to_string();
        if !token.is_empty() {
            return Ok(token);
        }
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).ok();
    }
    let token = random_token();
    std::fs::write(path, &token).with_context(|| format!("写入 {} 失败", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).ok();
    }
    Ok(token)
}

fn random_token() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("exv_{nanos:x}{:x}", std::process::id())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("shutdown signal received");
}
