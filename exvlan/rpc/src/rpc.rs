//! 桌面端 RPC 拨号：不依赖内核 crate 的最小 host runtime。
//!
//! 内核 crate 的 `proto::rpc::standalone` 依赖 `NativeHostRuntime`（自带 netns /
//! socket mark / FakeTCP 等能力）；GUI 只需要「能连 127.0.0.1 的 TCP + 解析域名」，
//! 因此在这里提供一个最小实现，避免把整颗内核链接进 GUI。

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use exvlan_core::connectivity::protocol::raw::TcpTunnelDialer;
use exvlan_core::host::dns::{DnsQuery, DnsResolver};
use exvlan_core::socket::tcp::{
    TcpConnectOptions, TcpSocketPurpose, VirtualTcpSocket, VirtualTcpSocketFactory,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// 基于 `tokio::net::TcpStream` 的最小 `VirtualTcpSocket` 实现。
pub struct TokioTcpSocket(tokio::net::TcpStream);

impl AsyncRead for TokioTcpSocket {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncWrite for TokioTcpSocket {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

impl VirtualTcpSocket for TokioTcpSocket {
    fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.0.local_addr()
    }

    fn peer_addr(&self) -> std::io::Result<SocketAddr> {
        self.0.peer_addr()
    }
}

/// GUI 侧 host runtime：直连 + 系统解析，不做 netns / socket mark / FakeTCP。
#[derive(Default)]
pub struct GuiHostRuntime;

impl GuiHostRuntime {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl VirtualTcpSocketFactory for GuiHostRuntime {
    type Socket = TokioTcpSocket;

    async fn connect_tcp(&self, options: TcpConnectOptions) -> Result<Self::Socket> {
        if options.purpose == TcpSocketPurpose::FakeTcp {
            anyhow::bail!("GUI RPC 不支持 FakeTCP");
        }
        let stream = tokio::net::TcpStream::connect(options.remote_addr).await?;
        stream.set_nodelay(true).ok();
        Ok(TokioTcpSocket(stream))
    }
}

#[async_trait]
impl DnsResolver for GuiHostRuntime {
    async fn resolve(&self, query: DnsQuery) -> Result<Vec<IpAddr>> {
        // 端口只用于 lookup_host 的接口形态，这里一律用 0。
        let addrs = tokio::net::lookup_host((query.host.as_str(), 0)).await?;
        let ips: Vec<IpAddr> = addrs.map(|addr| addr.ip()).collect();
        if ips.is_empty() {
            anyhow::bail!("无法解析主机: {}", query.host);
        }
        Ok(ips)
    }
}

/// 构造一个连向 `remote_url` 的 RPC 拨号器。
pub fn runtime_rpc_dialer(remote_url: url::Url) -> TcpTunnelDialer<GuiHostRuntime> {
    let runtime = Arc::new(GuiHostRuntime::new());
    let dns: Arc<dyn DnsResolver> = runtime.clone();
    TcpTunnelDialer::new(remote_url, runtime, dns)
}

#[cfg(test)]
mod tests {
    use super::*;
    use exvlan_core::connectivity::protocol::raw::TunnelDialer as _;

    #[test]
    fn dialer_builds_for_tcp_url() {
        let url: url::Url = "tcp://127.0.0.1:15999".parse().unwrap();
        let dialer = runtime_rpc_dialer(url.clone());
        assert_eq!(dialer.remote_url(), url);
    }

    #[tokio::test]
    async fn resolves_localhost() {
        let runtime = GuiHostRuntime::new();
        let ips = runtime
            .resolve(DnsQuery::new("localhost", Default::default()))
            .await
            .unwrap();
        assert!(!ips.is_empty());
    }

    #[tokio::test]
    async fn rejects_fake_tcp() {
        let runtime = GuiHostRuntime::new();
        let options = TcpConnectOptions::direct_connect("127.0.0.1:1".parse().unwrap())
            .with_purpose(TcpSocketPurpose::FakeTcp);
        assert!(runtime.connect_tcp(options).await.is_err());
    }
}
