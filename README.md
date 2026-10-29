# Exvlan

[![Github release](https://img.shields.io/github/v/tag/exvlan/exvlan)](https://github.com/exvlan/exvlan/releases)
[![GitHub](https://img.shields.io/github/license/exvlan/exvlan)](https://github.com/exvlan/exvlan/blob/main/LICENSE)
[![GitHub last commit](https://img.shields.io/github/last-commit/exvlan/exvlan)](https://github.com/exvlan/exvlan/commits/main)
[![GitHub issues](https://img.shields.io/github/issues/exvlan/exvlan)](https://github.com/exvlan/exvlan/issues)
[![GitHub Core Actions](https://github.com/exvlan/exvlan/actions/workflows/core.yml/badge.svg)](https://github.com/exvlan/exvlan/actions/workflows/core.yml)
[![GitHub GUI Actions](https://github.com/exvlan/exvlan/actions/workflows/studio.yml/badge.svg)](https://github.com/exvlan/exvlan/actions/workflows/studio.yml)
[![GitHub Test Actions](https://github.com/exvlan/exvlan/actions/workflows/test.yml/badge.svg)](https://github.com/exvlan/exvlan/actions/workflows/test.yml)
[![Ask DeepWiki](https://deepwiki.com/badge.svg)](https://deepwiki.com/exvlan/exvlan)

[简体中文](/README_CN.md) | [English](/README.md)

> ✨ A simple, secure, decentralized virtual private network solution powered by Rust and Tokio

<p align="center">
<img src="assets/config-page.png" width="300" alt="config page">
<img src="assets/running-page.png" width="300" alt="running page">
</p>

📚 **[Full Documentation](https://xvlan.era-x.cn/en/)** | 🖥️ **[Web Console](https://xvlan.era-x.cn/web)** | 📝 **[Download Releases](https://github.com/exvlan/exvlan/releases)** | 🧩 **[Third Party Tools](https://xvlan.era-x.cn/en/guide/installation_gui.html#third-party-graphical-interfaces)** | ❤️ **[Sponsor](#sponsor)**

## Features

### Core Features

- 🔒 **Decentralized**: Nodes are equal and independent, no centralized services required
- 🚀 **Easy to Use**: Multiple operation methods via web, client, and command line
- 🌍 **Cross-Platform**: Supports Win/MacOS/Linux/FreeBSD/Android and X86/ARM/MIPS architectures
- 🔐 **Secure**: AES-GCM or WireGuard encryption, prevents man-in-the-middle attacks

### Advanced Capabilities

- 🔌 **Efficient NAT Traversal**: Supports UDP and IPv6 traversal, works with NAT4-NAT4 networks
- 🌐 **Subnet Proxy**: Nodes can share subnets for other nodes to access
- 🔄 **Intelligent Routing**: Latency priority and automatic route selection for best network experience
- ⚡ **High Performance**: Zero-copy throughout the entire link, supports TCP/UDP/WSS/WG protocols

### Network Optimization

- 📊 **UDP Loss Resistance**: KCP/QUIC proxy optimizes latency and bandwidth in high packet loss environments
- 🔧 **Web Management**: Easy configuration and monitoring through web interface
- 🛠️ **Zero Config**: Simple deployment with statically linked executables

## Quick Start

### 📥 Installation

Choose the installation method that best suits your needs:

Linux (Recommended):

```bash
curl -fsSL "https://github.com/exvlan/exvlan/blob/main/script/install.sh?raw=true" | sudo bash -s install
```

Homebrew (MacOS/Linux):

```bash
brew tap era-club/exvlan
brew install --cask @exvlan/studio
```

Windows (Recommended, run with administrator privileges):

```powershell
irm "https://github.com/exvlan/exvlan/blob/main/script/install.ps1?raw=true" | iex
```

Install via cargo (Latest development version):

```bash
cargo install --git https://github.com/exvlan/exvlan.git @exvlan/studio
```

[Install pre-built binary](https://github.com/exvlan/exvlan/releases) (Recommended, All platforms supported)

[Install via Docker](https://doc.xvlan.era-x.cn/en/guide/installation.html#installation-methods)

[Install OpenWrt ipk package](https://github.com/exvlan/luci-app-exvlan)

Additional steps:

[One-Click Register Service](https://doc.xvlan.era-x.cn/en/guide/network/oneclick-install-as-service.html) (Automatically start when the system boots and run in the background)

### 🚀 Basic Usage

#### Quick Networking with Shared Nodes

Exvlan supports quick networking using shared public nodes. When you don't have a public IP, you can use the free shared nodes provided by the Exvlan community. Nodes will automatically attempt NAT traversal and establish P2P connections. When P2P fails, data will be relayed through shared nodes.

When using shared nodes, each node entering the network needs to provide the same `--network-name` and `--network-secret` parameters as the unique identifier of the network.

Taking two nodes as an example (Please use more complex network name to avoid conflicts):

1. Run on Node A:

```bash
# Run with administrator privileges
sudo exvlan -d --network-name abc --network-secret abc -p tcp://<SharedNodeIP>:11010
```

2. Run on Node B:

```bash
# Run with administrator privileges
sudo exvlan -d --network-name abc --network-secret abc -p tcp://<SharedNodeIP>:11010
```

After successful execution, you can check the network status using `exvlan-cli`:

```text
| ipv4         | hostname       | cost  | lat_ms | loss_rate | rx_bytes | tx_bytes | tunnel_proto | nat_type | id         | version         |
| ------------ | -------------- | ----- | ------ | --------- | -------- | -------- | ------------ | -------- | ---------- | --------------- |
| 10.126.126.1 | abc-1          | Local | *      | *         | *        | *        | udp          | FullCone | 439804259  | 26.410.20704-70e69a38~ |
| 10.126.126.2 | abc-2          | p2p   | 3.452  | 0         | 17.33 kB | 20.42 kB | udp          | FullCone | 390879727  | 26.410.20704-70e69a38~ |
|              | PublicServer_a | p2p   | 27.796 | 0.000     | 50.01 kB | 67.46 kB | tcp          | Unknown  | 3771642457 | 26.410.20704-70e69a38~ |
```

You can test connectivity between nodes:

```bash
# Test connectivity
ping 10.126.126.1
ping 10.126.126.2
```

Note: If you cannot ping through, it may be that the firewall is blocking incoming traffic. Please turn off the firewall or add allow rules.

To improve availability, you can connect to multiple shared nodes simultaneously:

```bash
# Connect to multiple shared nodes
sudo exvlan -d --network-name abc --network-secret abc -p tcp://<SharedNodeIP1>:11010 -p udp://<SharedNodeIP2>:11010
```

Once your network is set up successfully, you can easily configure it to start automatically on system boot. Refer to the [One-Click Register Service guide](https://xvlan.era-x.cn/en/guide/network/oneclick-install-as-service.html) for step-by-step instructions on registering Exvlan as a system service.

#### Decentralized Networking

Exvlan is fundamentally decentralized, with no distinction between server and client. As long as one device can communicate with any node in the virtual network, it can join the virtual network. Here's how to set up a decentralized network:

1. Start First Node (Node A):

```bash
# Start the first node
sudo exvlan -i 10.144.144.1
```

After startup, this node will listen on the following ports by default:

- TCP: 11010
- UDP: 11010
- WebSocket: 11011
- WebSocket SSL: 11012
- WireGuard: 11013

2. Connect Second Node (Node B):

```bash
# Connect to the first node using its public IP
sudo exvlan -i 10.144.144.2 -p udp://FIRST_NODE_PUBLIC_IP:11010
```

3. Verify Connection:

```bash
# Test connectivity
ping 10.144.144.2

# View connected peers
exvlan-cli peer

# View routing information
exvlan-cli route

# View local node information
exvlan-cli node
```

For more nodes to join the network, they can connect to any existing node in the network using the `-p` parameter:

```bash
# Connect to any existing node using its public IP
sudo exvlan -i 10.144.144.3 -p udp://ANY_EXISTING_NODE_PUBLIC_IP:11010
```

### 🔍 Advanced Features

#### Subnet Proxy

Assuming the network topology is as follows, Node B wants to share its accessible subnet 10.1.1.0/24 with other nodes:

```mermaid
flowchart LR

subgraph Node A Public IP 22.1.1.1
nodea[Exvlan<br/>10.144.144.1]
end

subgraph Node B
nodeb[Exvlan<br/>10.144.144.2]
end

id1[[10.1.1.0/24]]

nodea <--> nodeb <-.-> id1
```

To share a subnet, add the `-n` parameter when starting Exvlan:

```bash
# Share subnet 10.1.1.0/24 with other nodes
sudo exvlan -i 10.144.144.2 -n 10.1.1.0/24
```

Subnet proxy information will automatically sync to each node in the virtual network, and each node will automatically configure the corresponding route. You can verify the subnet proxy setup:

1. Check if the routing information has been synchronized (the proxy_cidrs column shows the proxied subnets):

```bash
# View routing information
exvlan-cli route
```

![Routing Information](/assets/image-3.png)

2. Test if you can access nodes in the proxied subnet:

```bash
# Test connectivity to proxied subnet
ping 10.1.1.2
```

#### WireGuard Integration

Exvlan can act as a WireGuard server, allowing any device with a WireGuard client (including iOS and Android) to access the Exvlan network. Here's an example setup:

```mermaid
flowchart LR

ios[[iPhone<br/>WireGuard Installed]]

subgraph Node A Public IP 22.1.1.1
nodea[Exvlan<br/>10.144.144.1]
end

subgraph Node B
nodeb[Exvlan<br/>10.144.144.2]
end

id1[[10.1.1.0/24]]

ios <-.-> nodea <--> nodeb <-.-> id1
```

1. Start Exvlan with WireGuard portal enabled:

```bash
# Listen on 0.0.0.0:11013 and use 10.14.14.0/24 subnet for WireGuard clients
sudo exvlan -i 10.144.144.1 --vpn-portal wg://0.0.0.0:11013/10.14.14.0/24
```

2. Get WireGuard client configuration:

```bash
# Get WireGuard client configuration
exvlan-cli vpn-portal
```

3. In the output configuration:
   - Set `Interface.Address` to an available IP from the WireGuard subnet
   - Set `Peer.Endpoint` to the public IP/domain of your Exvlan node
   - Import the modified configuration into your WireGuard client

#### Self-Hosted Public Shared Node

You can run your own public shared node to help other nodes discover each other. A public shared node is just a regular Exvlan network (with same network name and secret) that other networks can connect to.

To run a public shared node:

```bash
# No need to specify IPv4 address for public shared nodes
sudo exvlan --network-name mysharednode --network-secret mysharednode
```

## Related Projects

- [Exvlan](https://exvlan.cn/)：✨ A simple, secure, decentralized virtual private network solution powered by Rust and Tokio
- [ZeroTier](https://www.zerotier.com/): A global virtual network for connecting devices.
- [TailScale](https://tailscale.com/): A VPN solution aimed at simplifying network configuration.

### Contact Us

- 👥 **[Era Cloud Support](mailto:support@era-x.cn)**

## License

Exvlan is released under the [LGPL-3.0](https://github.com/exvlan/exvlan/blob/main/LICENSE).
