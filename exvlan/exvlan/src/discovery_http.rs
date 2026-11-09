//! `exvlan-cli discovery`：把本节点的监听地址用 HTTP 暴露出来。
//!
//! 客户端 `http://` / `https://` 发现协议会 GET 该地址，正文按**行**切分，
//! 每行一个完整 URL，随机选一个连接。这里返回的内容与 DNS TXT 发现保持同源。
//!
//! 设计要点：
//! - 零依赖（tokio + 手写最小 HTTP/1.1），避免给 CLI 引入整套 web 框架；
//! - `--advertise` 不填时用 STUN 探测到的公网 IP（NAT 环境下必须显式指定）；
//! - 可选 `--token`，要求 `?token=` 匹配，避免公网裸奔。

use std::time::Duration;

use anyhow::Context as _;
use exvlan_core::connectivity::stun::StunInfoProvider as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    pub bind: String,
    pub advertise: Option<String>,
    pub base_port: u16,
    pub schemes: Vec<String>,
    pub token: Option<String>,
}

/// 计算对外暴露的监听 URL 列表。
pub fn listener_urls(host: &str, base_port: u16, schemes: &[String]) -> Vec<String> {
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    let mut out = Vec::new();
    for scheme in schemes {
        match scheme.as_str() {
            "tcp" => out.push(format!("tcp://{host}:{base_port}")),
            "udp" => out.push(format!("udp://{host}:{base_port}")),
            "ws" => out.push(format!("ws://{host}:{}/", base_port + 1)),
            "wg" => out.push(format!("wg://{host}:{}", base_port + 1)),
            "quic" => out.push(format!("quic://{host}:{}", base_port + 2)),
            "wss" => out.push(format!("wss://{host}/ws")),
            other => tracing::warn!(scheme = other, "unknown discovery scheme, skipped"),
        }
    }
    out
}

/// 解析对外主机名：显式配置优先，否则用 STUN 探测到的公网 IP。
async fn resolve_advertise(explicit: Option<&str>) -> anyhow::Result<String> {
    if let Some(host) = explicit.map(str::trim).filter(|s| !s.is_empty()) {
        return Ok(host.to_string());
    }
    let collector = crate::common::stun::runtime_stun_info_collector(Default::default());
    let ip = tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let info = collector.get_stun_info();
            if let Some(ip) = info.public_ip.first() {
                // 形如 "1.2.3.4" 或 "1.2.3.4:1234"，只取主机部分
                return ip.split(':').next().unwrap_or(ip.as_str()).to_string();
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .context("STUN 探测超时，请用 --advertise 显式指定对外地址")?;
    Ok(ip)
}

/// 以 `--json` 方式打印一次结果（供 dns-sync 等脚本消费）。
pub fn print_json(cfg: &DiscoveryConfig, host: &str) -> anyhow::Result<()> {
    let urls = listener_urls(host, cfg.base_port, &cfg.schemes);
    let json = serde_json::json!({
        "advertise_host": host,
        "base_port": cfg.base_port,
        "listeners": urls,
    });
    println!("{}", serde_json::to_string_pretty(&json)?);
    Ok(())
}

/// 启动 HTTP 服务（阻塞直到进程退出）。
pub async fn serve(cfg: DiscoveryConfig) -> anyhow::Result<()> {
    let host = resolve_advertise(cfg.advertise.as_deref()).await?;
    let urls = listener_urls(&host, cfg.base_port, &cfg.schemes);
    let body = format!("{}\n", urls.join("\n"));
    let json_body = serde_json::json!({
        "advertise_host": host,
        "base_port": cfg.base_port,
        "listeners": urls,
    })
    .to_string();

    let listener = tokio::net::TcpListener::bind(&cfg.bind)
        .await
        .with_context(|| format!("监听 {} 失败", cfg.bind))?;
    tracing::info!(bind = %cfg.bind, %host, "discovery http listening");

    loop {
        let (mut socket, _peer) = match listener.accept().await {
            Ok(v) => v,
            Err(err) => {
                tracing::warn!(%err, "accept failed");
                continue;
            }
        };
        let body = body.clone();
        let json_body = json_body.clone();
        let token = cfg.token.clone();
        tokio::spawn(async move {
            let mut buf = [0u8; 2048];
            let n = match socket.read(&mut buf).await {
                Ok(0) | Err(_) => return,
                Ok(n) => n,
            };
            let req = String::from_utf8_lossy(&buf[..n]).into_owned();
            let path = req.split_whitespace().nth(1).unwrap_or("/");

            let (status, ctype, payload) = if let Some(expected) = token.as_deref() {
                let provided = path
                    .split_once("token=")
                    .map(|(_, v)| v.split('&').next().unwrap_or_default());
                if provided != Some(expected) {
                    (
                        "401 Unauthorized",
                        "text/plain; charset=utf-8",
                        "token required\n".to_string(),
                    )
                } else {
                    route(path, &body, &json_body)
                }
            } else {
                route(path, &body, &json_body)
            };

            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            let _ = socket.write_all(resp.as_bytes()).await;
            let _ = socket.flush().await;
        });
    }
}

fn route(path: &str, body: &str, json_body: &str) -> (&'static str, &'static str, String) {
    let path = path.split('?').next().unwrap_or(path);
    match path {
        "/" | "/discover" => ("200 OK", "text/plain; charset=utf-8", body.to_string()),
        "/json" => (
            "200 OK",
            "application/json; charset=utf-8",
            json_body.to_string(),
        ),
        "/healthz" => ("200 OK", "text/plain; charset=utf-8", "ok\n".to_string()),
        _ => (
            "404 Not Found",
            "text/plain; charset=utf-8",
            "not found\n".to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_follow_port_offsets() {
        let schemes = ["tcp", "udp", "ws", "wg", "quic", "wss"]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        let urls = listener_urls("example.com", 11010, &schemes);
        assert!(urls.contains(&"tcp://example.com:11010".to_string()));
        assert!(urls.contains(&"udp://example.com:11010".to_string()));
        assert!(urls.contains(&"ws://example.com:11011/".to_string()));
        assert!(urls.contains(&"wg://example.com:11011".to_string()));
        assert!(urls.contains(&"quic://example.com:11012".to_string()));
        assert!(urls.contains(&"wss://example.com/ws".to_string()));
    }

    #[test]
    fn ipv6_host_is_bracketed() {
        let urls = listener_urls("240b:4000::1", 11010, &["tcp".to_string()]);
        assert_eq!(urls, vec!["tcp://[240b:4000::1]:11010".to_string()]);
    }
}
