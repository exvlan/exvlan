import {
  createExvlanRuntime,
  type RuntimeWebSocket,
} from "@exvlan/runtime/adapter";
import type {
  ExvlanEvent,
  ExvlanInstance,
  ExvlanNetworkConfig,
} from "@exvlan/runtime";

export interface BrowserExvlanConfig extends ExvlanNetworkConfig {
  ipv4: string;
  peers: string | readonly string[];
}

export interface BrowserExvlanOptions {
  onEvent?: (event: ExvlanEvent) => void;
}

export async function createExvlanWithArtifact(
  artifact: BufferSource,
  config: BrowserExvlanConfig,
  options: BrowserExvlanOptions = {},
): Promise<ExvlanInstance> {
  return createExvlanRuntime({
    module: () => WebAssembly.compile(artifact),
    config: {
      profile: "browser",
      instanceId: crypto.randomUUID(),
      instanceName: config.instanceName ?? "exvlan-browser",
      networkName: config.networkName,
      networkSecret: config.networkSecret,
      encryption: config.encryption ?? true,
      ipv4: config.ipv4,
      peers:
        typeof config.peers === "string" ? [config.peers] : [...config.peers],
    },
    connectWebSocket: (url) =>
      new WebSocket(url) as unknown as RuntimeWebSocket,
    onEvent: options.onEvent,
  });
}
