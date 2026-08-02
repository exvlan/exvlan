# Core Module Documentation

## Overview

The core module of exvlan provides the fundamental functionality for creating and managing virtual networks, including tunnel establishment, peer-to-peer communication, proxy services, and network configuration.

## Module Structure

The core module is organized into several submodules, each responsible for specific functionality:

- **core**: Core functionality including CLI parsing, memory profiling, and main entry point
- **tunnel**: Various tunnel protocols implementation (TCP, UDP, WebSocket, WireGuard, QUIC)
- **peers**: Peer-to-peer communication, including connection management, encryption, and routing
- **instance**: Network instance management, including DNS servers, listeners, and proxy CIDR monitoring
- **gateway**: Proxy services implementation (TCP, UDP, ICMP, SOCKS5)
- **rpc_service**: RPC services for managing instances, peers, connectors, and more
- **common**: Common utilities and tools used across the module
- **connector**: Various connection methods (direct, DNS, HTTP, hole punching)
- **proto**: Protocol definitions and RPC implementations
- **peer_center**: Peer registration and discovery
- **web_client**: Web interface client implementation

## Key Features

### Tunnel Protocols
- **TCP**: Reliable TCP-based tunnel
- **UDP**: Fast UDP-based tunnel
- **WebSocket**: Tunnel over WebSocket protocol
- **WireGuard**: Integration with WireGuard VPN
- **QUIC**: Modern QUIC-based tunnel
- **Fake TCP**: TCP-like tunnel over UDP

### Proxy Services
- **TCP Proxy**: Proxy TCP traffic
- **UDP Proxy**: Proxy UDP traffic
- **ICMP Proxy**: Proxy ICMP traffic
- **SOCKS5 Proxy**: SOCKS5 protocol support
- **Fast SOCKS5**: High-performance SOCKS5 implementation

### Peer Management
- **Peer Discovery**: Find and connect to peers
- **Encryption**: Secure peer-to-peer communication
- **Routing**: Optimize traffic routing between peers
- **Relay**: Relay traffic through intermediate peers
- **ACL Filtering**: Access control for peers

### Instance Management
- **DNS Server**: Custom DNS server for the virtual network
- **Listeners**: Manage network listeners
- **Proxy CIDR Monitoring**: Monitor and update proxy CIDRs
- **Virtual NIC**: Virtual network interface management

### RPC Services
- **Instance Management**: Create, start, stop, and configure instances
- **Peer Management**: Add, remove, and manage peers
- **Connector Management**: Configure and manage connectors
- **Port Forwarding**: Set up port forwarding rules
- **ACL Management**: Manage access control lists
- **Credential Management**: Manage credentials for authentication

## Getting Started

### Prerequisites
- Rust programming language
- Tokio async runtime
- Various dependencies for specific features

### Installation

```bash
# Clone the repository
git clone <repository-url>

# Build the project
cd exvlan/core
cargo build

# Run the project
cargo run
```

### Configuration

The core module can be configured through command-line arguments or configuration files. Key configuration options include:

- **Network settings**: IP address, subnet, DNS settings
- **Tunnel settings**: Protocol, port, encryption
- **Peer settings**: Discovery method, authentication
- **Proxy settings**: CIDR ranges, port forwarding

### Usage Examples

#### Creating a Network Instance

```bash
# Create a new network instance
cargo run -- create --name my-network --ipv4 10.0.0.1/24

# Start the instance
cargo run -- start --name my-network

# Stop the instance
cargo run -- stop --name my-network
```

#### Adding Peers

```bash
# Add a peer
cargo run -- peer add --name my-network --address 192.168.1.100:8080

# List peers
cargo run -- peer list --name my-network

# Remove a peer
cargo run -- peer remove --name my-network --peer-id 12345
```

#### Configuring Port Forwarding

```bash
# Add port forwarding rule
cargo run -- port-forward add --name my-network --local-port 8080 --remote-addr 10.0.0.2:80

# List port forwarding rules
cargo run -- port-forward list --name my-network

# Remove port forwarding rule
cargo run -- port-forward remove --name my-network --local-port 8080
```

## API Documentation

The core module provides a comprehensive RPC API for managing network instances and their components. The API is divided into several services:

- **InstanceService**: Manage network instances
- **PeerService**: Manage peers
- **ConnectorService**: Manage connectors
- **PortForwardService**: Manage port forwarding
- **ACLService**: Manage access control lists
- **CredentialService**: Manage credentials
- **StatsService**: Get statistics and metrics

## Troubleshooting

### Common Issues

- **Connection Failures**: Check network connectivity, firewall settings, and peer addresses
- **DNS Resolution Issues**: Verify DNS server configuration and network settings
- **Performance Issues**: Check tunnel protocol settings, encryption configuration, and network bandwidth
- **Authentication Failures**: Verify credentials and authentication settings

### Logging

The core module uses structured logging to help diagnose issues. Logs can be configured through the command line or configuration files.

## Contributing

Contributions to the core module are welcome. Please follow the project's coding standards and submit pull requests for review.

## License

The core module is released under the [MIT License](LICENSE).