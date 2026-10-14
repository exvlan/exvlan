import coreBytes from "./generated/exvlan_core.wasm";
import { createExvlanWithArtifact } from "./create.js";
import type {
  ExvlanEvent,
  ExvlanInstance,
  ExvlanIpv4SocketAddress,
  ExvlanNetworkConfig,
  ExvlanOperationOptions,
  ExvlanState,
  ExvlanStatus,
  ExvlanTcpListener,
  ExvlanTcpReadResult,
  ExvlanTcpStream,
} from "@exvlan/runtime";
import type {
  BrowserExvlanConfig,
  BrowserExvlanOptions,
} from "./create.js";

export type {
  ExvlanEvent,
  ExvlanInstance,
  ExvlanIpv4SocketAddress,
  ExvlanNetworkConfig,
  ExvlanOperationOptions,
  ExvlanState,
  ExvlanStatus,
  ExvlanTcpListener,
  ExvlanTcpReadResult,
  ExvlanTcpStream,
};
export type { BrowserExvlanConfig, BrowserExvlanOptions };

export async function createExvlan(
  config: BrowserExvlanConfig,
  options: BrowserExvlanOptions = {},
): Promise<ExvlanInstance> {
  return createExvlanWithArtifact(coreBytes, config, options);
}
