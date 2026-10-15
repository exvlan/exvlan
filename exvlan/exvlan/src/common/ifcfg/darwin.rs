use std::net::Ipv4Addr;

use super::{Error, IfConfiguerTrait, cidr_to_subnet_mask, run_cmd, validate_ifname};
use async_trait::async_trait;
use cidr::{Ipv4Inet, Ipv6Inet};

pub struct MacIfConfiger {}
#[async_trait]
impl IfConfiguerTrait for MacIfConfiger {
    async fn add_ipv4_route(
        &self,
        name: &str,
        address: Ipv4Addr,
        cidr_prefix: u8,
        cost: Option<i32>,
    ) -> Result<(), Error> {
        validate_ifname(name)?;
        let addr = address.to_string();
        let mask = cidr_to_subnet_mask(cidr_prefix).to_string();
        let hop = cost.unwrap_or(7).to_string();
        run_cmd(
            "route",
            &[
                "-n",
                "add",
                &addr,
                "-netmask",
                &mask,
                "-interface",
                name,
                "-hopcount",
                &hop,
            ],
        )
        .await
    }

    async fn remove_ipv4_route(
        &self,
        name: &str,
        address: Ipv4Addr,
        cidr_prefix: u8,
    ) -> Result<(), Error> {
        validate_ifname(name)?;
        let addr = address.to_string();
        let mask = cidr_to_subnet_mask(cidr_prefix).to_string();
        run_cmd(
            "route",
            &["-n", "delete", &addr, "-netmask", &mask, "-interface", name],
        )
        .await
    }

    async fn add_ipv4_ip(
        &self,
        name: &str,
        address: Ipv4Addr,
        cidr_prefix: u8,
    ) -> Result<(), Error> {
        validate_ifname(name)?;
        let ip = format!("{}/{}", address, cidr_prefix);
        run_cmd("ifconfig", &[name, &ip, &address.to_string(), "up"]).await
    }

    async fn set_link_status(&self, name: &str, up: bool) -> Result<(), Error> {
        validate_ifname(name)?;
        run_cmd("ifconfig", &[name, if up { "up" } else { "down" }]).await
    }

    async fn remove_ip(&self, name: &str, ip: Option<Ipv4Inet>) -> Result<(), Error> {
        validate_ifname(name)?;
        if let Some(ip) = ip {
            run_cmd(
                "ifconfig",
                &[name, "inet", &ip.address().to_string(), "delete"],
            )
            .await
        } else {
            run_cmd("ifconfig", &[name, "inet", "delete"]).await
        }
    }

    async fn set_mtu(&self, name: &str, mtu: u32) -> Result<(), Error> {
        validate_ifname(name)?;
        run_cmd("ifconfig", &[name, "mtu", &mtu.to_string()]).await
    }

    async fn add_ipv6_ip(
        &self,
        name: &str,
        address: std::net::Ipv6Addr,
        cidr_prefix: u8,
    ) -> Result<(), Error> {
        validate_ifname(name)?;
        let ip = format!("{}/{}", address, cidr_prefix);
        run_cmd("ifconfig", &[name, "inet6", &ip, "add"]).await
    }

    async fn remove_ipv6(&self, name: &str, ip: Option<Ipv6Inet>) -> Result<(), Error> {
        validate_ifname(name)?;
        if let Some(ip) = ip {
            run_cmd(
                "ifconfig",
                &[name, "inet6", &ip.address().to_string(), "delete"],
            )
            .await
        } else {
            // Remove all IPv6 addresses is more complex on macOS, just succeed
            Ok(())
        }
    }

    async fn add_ipv6_route(
        &self,
        name: &str,
        address: std::net::Ipv6Addr,
        cidr_prefix: u8,
        cost: Option<i32>,
    ) -> Result<(), Error> {
        validate_ifname(name)?;
        let net = format!("{}/{}", address, cidr_prefix);
        if let Some(cost) = cost {
            let hop = cost.to_string();
            run_cmd(
                "route",
                &[
                    "-n",
                    "add",
                    "-inet6",
                    &net,
                    "-interface",
                    name,
                    "-hopcount",
                    &hop,
                ],
            )
            .await
        } else {
            run_cmd("route", &["-n", "add", "-inet6", &net, "-interface", name]).await
        }
    }

    async fn remove_ipv6_route(
        &self,
        name: &str,
        address: std::net::Ipv6Addr,
        cidr_prefix: u8,
    ) -> Result<(), Error> {
        validate_ifname(name)?;
        let net = format!("{}/{}", address, cidr_prefix);
        run_cmd(
            "route",
            &["-n", "delete", "-inet6", &net, "-interface", name],
        )
        .await
    }
}
