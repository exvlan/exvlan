# `@exvlan/browser`

Run an outbound Exvlan instance and its TCP data plane directly in a browser.
The package embeds the matching Exvlan WebAssembly artifact in its JavaScript
entry point; applications do not configure a bundler loader, compile Rust, load
WASI, or work with Guest handles.

## Install

```sh
pnpm add @exvlan/browser
```

## Connect to an Exvlan network

```ts
import { createExvlan } from "@exvlan/browser";

const exvlan = await createExvlan({
  networkName: "office",
  networkSecret: "secret",
  ipv4: "10.144.0.10/24",
  peers: "wss://relay.example.com/",
  encryption: true,
}, {
  onEvent(event) {
    console.log(event.kind, event.message);
  },
});
```

`createExvlan()` resolves after the Exvlan Instance is running. When the
peer is an `@exvlan/cloudflare` relay, `networkName` and `networkSecret` must
match the relay configuration. Use `ws://` for local development and `wss://`
for a deployed Worker.

## Use the TCP data plane

```ts
const status = await exvlan.status();
console.log(status.state, status.connections);

const stream = await exvlan.connectTcp("10.144.0.20:8080", {
  timeout: 10_000,
});
await stream.write(new TextEncoder().encode("hello"));
const response = await stream.read();
console.log(new TextDecoder().decode(response.data));
await stream.close();
await exvlan.close();
```

The Browser Adapter supports `ws://` and `wss://` peers and an overlay IPv4 TCP
data plane. It does not expose native listeners, TUN, STUN, or hole punching.
The browser must support WebAssembly JSPI.

## Run the complete example

The repository contains a standalone Browser-to-Cloudflare example that only
uses the public package entries:

[`exvlan-js/examples/web`](https://github.com/exvlan/exvlan/tree/main/exvlan-js/examples/web)

It includes the Worker configuration, local secret setup, Browser UI, health
check, deployment commands, and the expected `peer_added` result.
