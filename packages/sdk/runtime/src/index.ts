export type ExvlanState =
  | "created"
  | "starting"
  | "running"
  | "stopping"
  | "stopped";

export interface ExvlanNetworkConfig {
  networkName: string;
  networkSecret: string;
  instanceName?: string;
  encryption?: boolean;
}

export interface ExvlanEvent {
  kind: string;
  message: string;
}

export interface ExvlanStatus {
  state: ExvlanState;
  connections: number;
}

export interface ExvlanOperationOptions {
  /** Timeout in milliseconds. Omit to wait indefinitely. */
  timeout?: number;
}

export interface ExvlanIpv4SocketAddress {
  ipv4: string;
  port: number;
}

export interface ExvlanTcpReadResult {
  data: Uint8Array;
  eof: boolean;
}

export interface ExvlanTcpStream {
  readonly localAddress: ExvlanIpv4SocketAddress;
  readonly peerAddress: ExvlanIpv4SocketAddress;
  read(maxLength?: number): Promise<ExvlanTcpReadResult>;
  write(data: Uint8Array): Promise<number>;
  shutdownWrite(): Promise<void>;
  close(): Promise<void>;
}

export interface ExvlanTcpListener {
  readonly localAddress: ExvlanIpv4SocketAddress;
  accept(options?: ExvlanOperationOptions): Promise<ExvlanTcpStream>;
  close(): Promise<void>;
}

export interface ExvlanInstance {
  connectTcp(
    address: string,
    options?: ExvlanOperationOptions,
  ): Promise<ExvlanTcpStream>;
  listenTcp(
    port: number,
    options?: ExvlanOperationOptions,
  ): Promise<ExvlanTcpListener>;
  status(): Promise<ExvlanStatus>;
  close(): Promise<void>;
}
