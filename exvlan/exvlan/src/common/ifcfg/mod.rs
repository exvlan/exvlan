#![cfg_attr(
    not(any(feature = "public-ipv6-provider", feature = "tun")),
    allow(dead_code)
)]

#[cfg(any(
    all(target_os = "macos", not(feature = "macos-ne")),
    target_os = "freebsd"
))]
mod darwin;
#[cfg(all(target_os = "linux", feature = "linux-netlink"))]
mod netlink;
#[cfg(all(target_os = "linux", feature = "linux-netlink"))]
mod netlink_wire;
#[cfg(target_os = "windows")]
mod win;
#[cfg(target_os = "windows")]
mod windows;

use std::net::{Ipv4Addr, Ipv6Addr};

use async_trait::async_trait;
use cidr::{Ipv4Inet, Ipv6Inet};
#[cfg(any(
    all(target_os = "macos", not(feature = "macos-ne")),
    target_os = "freebsd"
))]
use tokio::process::Command;

use super::error::Error;

#[async_trait]
pub trait IfConfiguerTrait: Send + Sync {
    async fn add_ipv4_route(
        &self,
        _name: &str,
        _address: Ipv4Addr,
        _cidr_prefix: u8,
        _cost: Option<i32>,
    ) -> Result<(), Error> {
        Ok(())
    }
    async fn remove_ipv4_route(
        &self,
        _name: &str,
        _address: Ipv4Addr,
        _cidr_prefix: u8,
    ) -> Result<(), Error> {
        Ok(())
    }
    async fn add_ipv4_ip(
        &self,
        _name: &str,
        _address: Ipv4Addr,
        _cidr_prefix: u8,
    ) -> Result<(), Error> {
        Ok(())
    }
    async fn add_ipv6_route(
        &self,
        _name: &str,
        _address: Ipv6Addr,
        _cidr_prefix: u8,
        _cost: Option<i32>,
    ) -> Result<(), Error> {
        Ok(())
    }
    async fn remove_ipv6_route(
        &self,
        _name: &str,
        _address: Ipv6Addr,
        _cidr_prefix: u8,
    ) -> Result<(), Error> {
        Ok(())
    }
    async fn add_ipv6_ip(
        &self,
        _name: &str,
        _address: Ipv6Addr,
        _cidr_prefix: u8,
    ) -> Result<(), Error> {
        Ok(())
    }
    async fn set_link_status(&self, _name: &str, _up: bool) -> Result<(), Error> {
        Ok(())
    }
    async fn remove_ip(&self, _name: &str, _ip: Option<Ipv4Inet>) -> Result<(), Error> {
        Ok(())
    }
    async fn remove_ipv6(&self, _name: &str, _ip: Option<Ipv6Inet>) -> Result<(), Error> {
        Ok(())
    }
    async fn wait_interface_show(&self, _name: &str) -> Result<(), Error> {
        return Ok(());
    }
    async fn set_mtu(&self, _name: &str, _mtu: u32) -> Result<(), Error> {
        Ok(())
    }
}

#[cfg(any(
    all(target_os = "macos", not(feature = "macos-ne")),
    target_os = "freebsd"
))]
fn cidr_to_subnet_mask(prefix_length: u8) -> Ipv4Addr {
    if prefix_length > 32 {
        panic!("Invalid CIDR prefix length");
    }

    let subnet_mask: u32 = (!0u32)
        .checked_shl(32 - u32::from(prefix_length))
        .unwrap_or(0);
    Ipv4Addr::new(
        ((subnet_mask >> 24) & 0xFF) as u8,
        ((subnet_mask >> 16) & 0xFF) as u8,
        ((subnet_mask >> 8) & 0xFF) as u8,
        (subnet_mask & 0xFF) as u8,
    )
}

#[cfg(any(
    all(target_os = "macos", not(feature = "macos-ne")),
    target_os = "freebsd"
))]
async fn run_cmd(program: &str, args: &[&str]) -> Result<(), Error> {
    let output = Command::new(program).args(args).output().await?;
    let stdout = String::from_utf8_lossy(output.stdout.as_slice()).to_string();
    let stderr = String::from_utf8_lossy(output.stderr.as_slice()).to_string();

    let ec = output.status.code();
    let succ = output.status.success();
    tracing::info!(program, ?args, ?ec, ?succ, ?stdout, ?stderr, "run cmd");

    if !output.status.success() {
        return Err(Error::ShellCommandError(stdout + &stderr));
    }
    Ok(())
}

/// 校验网卡名，防止将不可信（如配置/服务端下发）的接口名用于系统命令。
#[cfg(any(
    all(target_os = "macos", not(feature = "macos-ne")),
    target_os = "freebsd"
))]
pub(crate) fn validate_ifname(name: &str) -> Result<(), Error> {
    if name.is_empty()
        || name.len() > 15
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '@'))
    {
        return Err(Error::InvalidInterfaceName(name.to_string()));
    }
    Ok(())
}

pub struct DummyIfConfiger {}
#[async_trait]
impl IfConfiguerTrait for DummyIfConfiger {}

#[cfg(all(target_os = "linux", feature = "linux-netlink"))]
pub type IfConfiger = netlink::NetlinkIfConfiger;
#[cfg(all(target_os = "linux", not(feature = "linux-netlink")))]
pub type IfConfiger = DummyIfConfiger;

#[cfg(any(
    all(target_os = "macos", not(feature = "macos-ne")),
    target_os = "freebsd"
))]
pub type IfConfiger = darwin::MacIfConfiger;

#[cfg(target_os = "windows")]
pub type IfConfiger = windows::WindowsIfConfiger;

#[cfg(not(any(
    all(target_os = "macos", not(feature = "macos-ne")),
    target_os = "linux",
    target_os = "windows",
    target_os = "freebsd",
)))]
pub type IfConfiger = DummyIfConfiger;

#[cfg(target_os = "windows")]
pub use windows::RegistryManager;

#[cfg(all(target_os = "linux", feature = "linux-netlink"))]
pub(crate) use netlink_wire::RouteMessage;
#[cfg(all(
    target_os = "linux",
    feature = "linux-netlink",
    feature = "public-ipv6-provider"
))]
pub(crate) use netlink_wire::RouteType;

#[cfg(all(target_os = "linux", feature = "linux-netlink"))]
pub(crate) fn list_ipv6_route_messages() -> Result<Vec<RouteMessage>, Error> {
    netlink::NetlinkIfConfiger::list_ipv6_route_messages()
}

#[cfg(all(target_os = "linux", feature = "linux-netlink"))]
pub(crate) fn get_interface_index(name: &str) -> Result<u32, Error> {
    netlink::NetlinkIfConfiger::get_interface_index(name)
}

#[cfg(all(target_os = "linux", feature = "linux-netlink"))]
pub(crate) fn add_ipv6_ndp_proxy(name: &str, address: Ipv6Addr) -> Result<(), Error> {
    netlink::NetlinkIfConfiger::add_ipv6_ndp_proxy(name, address)
}

#[cfg(all(target_os = "linux", feature = "linux-netlink"))]
pub(crate) fn remove_ipv6_ndp_proxy(name: &str, address: Ipv6Addr) -> Result<(), Error> {
    netlink::NetlinkIfConfiger::remove_ipv6_ndp_proxy(name, address)
}

#[cfg(all(target_os = "linux", feature = "linux-netlink"))]
pub(crate) fn list_ipv6_ndp_proxy(
    name: &str,
) -> Result<std::collections::BTreeSet<Ipv6Addr>, Error> {
    netlink::NetlinkIfConfiger::list_ipv6_ndp_proxy(name)
}
