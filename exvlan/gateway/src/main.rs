//! EXVlan 控制面网关（HTTP/JSON + WebSocket）。
//!
//! 定位：**薄**。它是 GUI / CLI / agent / （将来的）Web UI 的共同入口，
//! 只做「鉴权 + 路由 + 转发」，业务判断全部在被调方：
//!
//! - 服务相关（policy / kernel / 安装）→ `exvlan-service`（IPC）
//! - 体检 / 诊断 → `exvlan-control::doctor`（同一份 findings）
//!
//! 默认只监听 `127.0.0.1`，鉴权用 Bearer token（`<user>/gateway-token`，0600）。
//! 远端访问需要显式开启 mesh 监听（opt-in），属后续版本。

mod api;
mod kernel;

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

/// 绑定重试次数：Windows 开机后 mesh 虚拟地址（如 `10.254.1.22`）通常比
/// 服务晚几秒才就绪，首次绑定会拿到 `WSAEADDRNOTAVAIL`（10049）。
const BIND_RETRY_ATTEMPTS: u32 = 5;
/// 重试间隔：5 次 × 2s ≈ 8s 的内部等待窗口。
///
/// 与 supervisor 的关系是**顺序**而非叠加：watchdog 每 5s 只看进程是否存活，
/// 存活就不动；本进程退出后它才 sleep 退避再来拉起。所以两边不会同时等待。
const BIND_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(2);

/// 只有「地址还不可用」才值得等（网卡/虚拟 IP 未就绪）。
///
/// 端口被占（`AddrInUse` / WSAEADDRINUSE）与权限不足（`PermissionDenied` /
/// WSAEACCES）是真冲突，重试只会拖慢失败；交给 supervisor 与调用方快速看到错误。
fn is_transient_bind_error(err: &std::io::Error) -> bool {
    if err.kind() == std::io::ErrorKind::AddrNotAvailable {
        return true;
    }
    // 兜底：老工具链 / 特定平台未把错误码映射到 AddrNotAvailable 时按原始码判定。
    #[cfg(windows)]
    if err.raw_os_error() == Some(10049) {
        return true;
    }
    false
}

/// 带短退避的绑定：仅对「地址不可用」重试，其余错误立即上抛（保留原始 io 错误）。
async fn bind_with_retry(addr: SocketAddr) -> anyhow::Result<TcpListener> {
    let mut attempt: u32 = 1;
    loop {
        match TcpListener::bind(addr).await {
            Ok(listener) => return Ok(listener),
            Err(err) => {
                let transient = is_transient_bind_error(&err);
                if !transient || attempt >= BIND_RETRY_ATTEMPTS {
                    let ctx = if transient {
                        format!(
                            "绑定 {addr} 失败：地址仍不可用（已重试 {} 次，约 {}s）",
                            attempt - 1,
                            u64::from(attempt - 1) * BIND_RETRY_DELAY.as_secs()
                        )
                    } else {
                        format!("绑定 {addr} 失败（不可重试：{}）", err.kind())
                    };
                    return Err(err).context(ctx);
                }
                tracing::warn!(
                    "bind {addr} 失败（第 {attempt}/{BIND_RETRY_ATTEMPTS} 次，地址暂不可用，{}s 后重试）: {err}",
                    BIND_RETRY_DELAY.as_secs()
                );
                tokio::time::sleep(BIND_RETRY_DELAY).await;
                attempt += 1;
            }
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "exvlan_gateway=info,tower_http=warn".into()),
        )
        .init();

    let user = exvlan_control::paths::user_paths();
    let token_file = std::env::var("EXVLAN_GATEWAY_TOKEN_FILE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| user.data.join("gateway-token"));

    let cfg = Config {
        bind: env_or("EXVLAN_GATEWAY_BIND", "127.0.0.1"),
        port: env_or(
            "EXVLAN_GATEWAY_PORT",
            &exvlan_control::DEFAULT_GATEWAY_PORT.to_string(),
        )
        .parse()
        .context("EXVLAN_GATEWAY_PORT 必须是端口号")?,
        token_file,
    };

    let token = load_or_create_token(&cfg.token_file)?;
    tracing::info!(
        "token store: {}",
        exvlan_control::tokens::Store::path().display()
    );

    let app: Router = api::router(cfg.clone(), token);
    let addr: SocketAddr = format!("{}:{}", cfg.bind, cfg.port)
        .parse()
        .context("非法监听地址")?;
    let listener = bind_with_retry(addr).await?;
    tracing::info!(
        "gateway listening on {}:{} (token file: {})",
        cfg.bind,
        cfg.port,
        cfg.token_file.display()
    );

    // 需要 ConnectInfo（配对码兑换要按来源 IP 限速 + 绑定）。
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
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
    tokio::select! {
        _ = tokio::signal::ctrl_c() => tracing::info!("shutdown signal received"),
        _ = parent_exit() => tracing::info!("parent exited (stdin closed), shutting down"),
    }
}

/// 由服务托管时，父进程把我们的 stdin 接成管道：父进程一退出，管道就 EOF。
///
/// 比依赖 SIGTERM 可靠得多 —— macOS launchd / Windows SCM 杀掉父进程时，
/// 信号未必传到子进程；靠 EOF 则不会被漏掉（也就不会留下占端口的孤儿 gateway）。
async fn parent_exit() {
    use tokio::io::AsyncReadExt as _;
    if std::env::var("EXVLAN_GATEWAY_PARENT_STDIN").as_deref() != Ok("1") {
        std::future::pending::<()>().await;
        return;
    }
    let mut stdin = tokio::io::stdin();
    let mut buf = [0u8; 64];
    loop {
        match stdin.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            // 父进程本就不会写数据；写了也忽略。
            Ok(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Error, ErrorKind};

    #[test]
    fn retries_only_when_address_is_not_available() {
        // WSAEADDRNOTAVAIL / EADDRNOTAVAIL：网卡或虚拟 IP 还没就绪。
        assert!(is_transient_bind_error(&Error::from(
            ErrorKind::AddrNotAvailable
        )));
        #[cfg(windows)]
        assert!(is_transient_bind_error(&Error::from_raw_os_error(10049)));
    }

    #[test]
    fn hard_conflicts_fail_fast() {
        assert!(!is_transient_bind_error(&Error::from(ErrorKind::AddrInUse)));
        assert!(!is_transient_bind_error(&Error::from(
            ErrorKind::PermissionDenied
        )));
        assert!(!is_transient_bind_error(&Error::from(
            ErrorKind::InvalidInput
        )));
        #[cfg(windows)]
        {
            assert!(!is_transient_bind_error(&Error::from_raw_os_error(10048)));
            assert!(!is_transient_bind_error(&Error::from_raw_os_error(10013)));
        }
    }

    #[test]
    fn retry_window_stays_below_supervisor_cadence() {
        // supervisor 每 5s 探一次、退出后才按 1→2→5→10s 退避；内部窗口封顶在最大退避量级，
        // 保证「内部重试先跑完、退出后才轮到 supervisor」，不会两边同时等待。
        let window = BIND_RETRY_DELAY * (BIND_RETRY_ATTEMPTS - 1);
        assert!(window <= std::time::Duration::from_secs(10));
        assert!(window > std::time::Duration::from_secs(0));
    }
}
