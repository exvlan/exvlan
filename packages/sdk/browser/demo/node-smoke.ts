import { readFile } from "node:fs/promises";

import {
  createExvlanRuntime,
  type RuntimeWebSocket,
} from "@exvlan/runtime/adapter";
import {
  connectWithRetry,
  deadline,
  probeHttpMarker,
  SMOKE_RETRY_MILLISECONDS,
} from "./smoke-shared";

const relayUrl = process.env.EXVLAN_BROWSER_RELAY ?? "ws://127.0.0.1:11011/";
const networkName = process.env.EXVLAN_BROWSER_NETWORK ?? "browser-smoke";
const networkSecret = process.env.EXVLAN_BROWSER_SECRET ?? "browser-smoke-test";
const ipv4 = process.env.EXVLAN_BROWSER_IPV4 ?? "10.144.144.2";
const moduleBytes = await readFile(
  process.env.EXVLAN_BROWSER_WASM ??
    new URL("./exvlan_core.wasm", import.meta.url),
);
const copiedModuleBytes = new Uint8Array(moduleBytes.byteLength);
copiedModuleBytes.set(moduleBytes);
const module = new WebAssembly.Module(copiedModuleBytes);

let peerAdded = false;
const runtime = await createExvlanRuntime({
  module,
  config: {
    profile: "browser",
    instanceId:
      process.env.EXVLAN_BROWSER_INSTANCE_ID ?? crypto.randomUUID(),
    instanceName: "browser-node-smoke",
    networkName,
    networkSecret,
    encryption: true,
    ipv4: `${ipv4}/24`,
    peers: [relayUrl],
  },
  connectWebSocket: (url) =>
    new WebSocket(url) as unknown as RuntimeWebSocket,
  onEvent: (event) => {
    console.log(JSON.stringify({ event: "exvlan_core_event", ...event }));
    if (event.kind === "peer_added") {
      peerAdded = true;
    }
  },
});

const timeoutAt = deadline();
while (Date.now() < timeoutAt) {
  const health = await runtime.status();
  if (peerAdded) {
    const target = process.env.EXVLAN_BROWSER_TCP_TARGET;
    if (target !== undefined) {
      const stream = await connectWithRetry(
        () => runtime.connectTcp(target, { timeout: 1_000 }),
        timeoutAt,
      );
      const host = target.slice(0, target.lastIndexOf(":"));
      await probeHttpMarker(stream, host, "browser-data-plane-ok");
      console.log(
        JSON.stringify({ event: "browser_smoke_data_plane", target }),
      );
    }
    console.log(JSON.stringify({ event: "browser_smoke_connected", health }));
    process.exit(0);
  }
  await new Promise((resolve) => setTimeout(resolve, SMOKE_RETRY_MILLISECONDS));
}

throw new Error(`Exvlan did not connect to ${relayUrl}`);
