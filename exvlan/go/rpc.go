package host

import internalhost "github.com/exvlan/exvlan/exvlan-go/internal/host"

// PeerInfo describes one peer visible to an Exvlan instance.
type PeerInfo = internalhost.PeerInfo

// Route describes one route visible to an Exvlan instance.
type Route = internalhost.Route

// NodeInfo describes this Exvlan instance.
type NodeInfo = internalhost.NodeInfo
