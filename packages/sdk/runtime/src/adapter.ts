import type {
  ExvlanEvent,
  ExvlanInstance,
  ExvlanOperationOptions,
  ExvlanState,
  ExvlanStatus,
  ExvlanTcpListener,
  ExvlanTcpStream,
} from "./index.js";
import { encodeRuntimeConfig, type RuntimeInstanceConfig } from "./config.js";
import {
  ExvlanRuntime,
  type HostTunnelMetadata,
} from "./runtime.js";
import type { RuntimeWebSocket } from "./websocket-host.js";

export type {
  HostTunnelMetadata,
  RuntimeInstanceConfig,
  RuntimeWebSocket,
};

export interface RuntimeAdapter extends ExvlanInstance {
  canAcceptWebSocket(): boolean;
  acceptWebSocket(
    socket: RuntimeWebSocket,
    metadata: HostTunnelMetadata,
  ): Promise<void>;
}

export interface CreateRuntimeOptions {
  module: WebAssembly.Module | (() => Promise<WebAssembly.Module>);
  config: RuntimeInstanceConfig;
  connectWebSocket?: (url: string) => RuntimeWebSocket;
  onEvent?: (event: ExvlanEvent) => void;
}

export async function createExvlanRuntime(
  options: CreateRuntimeOptions,
): Promise<RuntimeAdapter> {
  const config = encodeRuntimeConfig(options.config);
  const module =
    typeof options.module === "function"
      ? await options.module()
      : options.module;
  const runtime = new ExvlanRuntime(
    module,
    config,
    options.connectWebSocket,
    options.onEvent,
  );
  try {
    await runtime.ready;
  } catch (error) {
    await runtime.stop().catch(() => {});
    throw new Error("failed to start Exvlan", { cause: error });
  }

  return {
    connectTcp: (
      address: string,
      operation?: ExvlanOperationOptions,
    ): Promise<ExvlanTcpStream> => {
      const target = parseTcpAddress(address);
      return runtime.connectTcp(target.ipv4, target.port, operation?.timeout);
    },
    listenTcp: (
      port: number,
      operation?: ExvlanOperationOptions,
    ): Promise<ExvlanTcpListener> =>
      runtime.bindTcp(port, operation?.timeout).then((listener) => ({
        localAddress: listener.localAddress,
        accept: (acceptOptions) =>
          listener.accept(acceptOptions?.timeout),
        close: () => listener.close(),
      })),
    status: async (): Promise<ExvlanStatus> => {
      const health = await runtime.health();
      return {
        state: stateName(health.state),
        connections: health.connections,
      };
    },
    close: () => runtime.stop(),
    canAcceptWebSocket: () => runtime.canAcceptWebSocket(),
    acceptWebSocket: (socket, metadata) =>
      runtime.acceptWebSocket(socket, metadata),
  };
}

function parseTcpAddress(address: string): { ipv4: string; port: number } {
  const separator = address.lastIndexOf(":");
  if (separator <= 0) {
    throw new Error(`invalid IPv4 TCP address: ${address}`);
  }
  const ipv4 = address.slice(0, separator);
  const port = Number(address.slice(separator + 1));
  if (!Number.isInteger(port) || port < 1 || port > 65_535) {
    throw new Error(`invalid TCP port in address: ${address}`);
  }
  return { ipv4, port };
}

function stateName(state: number): ExvlanState {
  switch (state) {
    case 0:
      return "created";
    case 1:
      return "starting";
    case 2:
      return "running";
    case 3:
      return "stopping";
    case 4:
      return "stopped";
    default:
      throw new Error(`guest returned unknown instance state ${state}`);
  }
}
