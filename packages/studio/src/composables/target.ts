// 运行方式（runtime）× 目标（target）模型。
//
// 两者**正交**：
//   runtime = service | normal | off   决定本机内核怎么活
//   target  = local | <remote id>      决定界面连哪个 RPC
//
// 「远程 + 本地普通模式」是允许的组合：本地内核照常跑，界面只是连到别处。
// 切换 target **不会**停本机内核，也不会断开已有会话（引用计数在 Rust 侧）。

import { persistUiStateNow } from "./persist";

export type RuntimeKind = "service" | "normal" | "off";

/**
 * 远端的两种接法：
 * - `direct`：直连对端内核 RPC（1995，无鉴权，靠对端 RPC 白名单兜底）
 * - `gateway`：走对端 gateway（2016，Bearer token；token 由对端配对签发）
 *
 * 两者并存、由用户显式选择 —— 不做静默降级（权限与暴露面不同，
 * 静默切换会让「我到底连到了哪」变模糊）。
 */
export type RemoteMode = "direct" | "gateway";

export interface RemoteTarget {
  id: string;
  /** 显示名（缺省用 host:port） */
  name: string;
  host: string;
  port: number;
  /** 缺省 = direct（旧数据没有这个字段） */
  mode?: RemoteMode;
}

export const LOCAL_TARGET = "local";
export const DEFAULT_RPC_PORT = 1995;
/** gateway 默认端口（`service.toml [gateway].port`）。 */
export const DEFAULT_GATEWAY_PORT = 2016;

export function remoteMode(target: RemoteTarget): RemoteMode {
  return target.mode === "gateway" ? "gateway" : "direct";
}

const KEY_RUNTIME = "runtime";
const KEY_TARGET = "target";
const KEY_REMOTES = "remotes";

export function defaultRuntime(): RuntimeKind {
  // 7.3 起默认服务模式：开机自启、关界面继续跑。
  return "service";
}

/** 运行方式（`prefs.toml` 的 `runtime`，唯一权威）。 */
export function loadRuntime(): RuntimeKind {
  const raw = localStorage.getItem(KEY_RUNTIME);
  if (raw === "service" || raw === "normal" || raw === "off") {
    return raw;
  }
  return defaultRuntime();
}

export async function saveRuntime(runtime: RuntimeKind): Promise<void> {
  await persistUiStateNow(KEY_RUNTIME, runtime);
}

export function loadRemotes(): RemoteTarget[] {
  const raw = localStorage.getItem(KEY_REMOTES);
  if (!raw) {
    return [];
  }
  try {
    const list = JSON.parse(raw);
    if (!Array.isArray(list)) return [];
    return (
      list
        .filter((item): item is RemoteTarget => {
          return (
            item &&
            typeof item.id === "string" &&
            typeof item.host === "string" &&
            item.host.length > 0
          );
        })
        // 直连：端口固定 1995（由对端自动发现）；gateway：默认 2016，可显式改。
        // 归一化历史数据里的手误端口，避免用户去猜。
        .map((item) =>
          item.mode === "gateway"
            ? { ...item, port: item.port || DEFAULT_GATEWAY_PORT }
            : { ...item, port: DEFAULT_RPC_PORT, mode: "direct" as const },
        )
    );
  } catch {
    return [];
  }
}

export async function saveRemotes(remotes: RemoteTarget[]): Promise<void> {
  await persistUiStateNow(KEY_REMOTES, JSON.stringify(remotes));
}

export function loadTargetId(): string {
  const raw = localStorage.getItem(KEY_TARGET);
  if (raw) {
    return raw;
  }
  return LOCAL_TARGET;
}

export async function saveTargetId(id: string): Promise<void> {
  await persistUiStateNow(KEY_TARGET, id);
}

export function findRemote(id: string): RemoteTarget | undefined {
  if (id === LOCAL_TARGET) return undefined;
  return loadRemotes().find((r) => r.id === id);
}

/** `tcp://host:port` */
export function remoteUrl(remote: RemoteTarget): string {
  const port = remote.port || DEFAULT_RPC_PORT;
  const host =
    remote.host.includes(":") && !remote.host.startsWith("[") ? `[${remote.host}]` : remote.host;
  return `tcp://${host}:${port}`;
}

export function targetLabel(id: string, localLabel: string): string {
  if (id === LOCAL_TARGET) return localLabel;
  const remote = findRemote(id);
  if (!remote) return id;
  return remote.name || remoteAddress(remote);
}

/** `10.254.1.6:2016`（只用于显示；直连目标不显示端口以免误导）。 */
export function remoteAddress(remote: RemoteTarget): string {
  const host =
    remote.host.includes(":") && !remote.host.startsWith("[") ? `[${remote.host}]` : remote.host;
  return remoteMode(remote) === "gateway" ? `${host}:${remote.port || DEFAULT_GATEWAY_PORT}` : host;
}

export function newRemote(
  host = "",
  port = DEFAULT_RPC_PORT,
  mode: RemoteMode = "direct",
): RemoteTarget {
  return {
    id: `rm-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`,
    name: "",
    host,
    port: mode === "gateway" ? DEFAULT_GATEWAY_PORT : port,
    mode,
  };
}

/** 允许在地址里出现的端口：当前默认值 + 历史默认值（便于粘贴旧地址）。 */
const ACCEPTED_PORTS = new Set([DEFAULT_RPC_PORT, 15999]);

/**
 * 解析「远程目标主机」。端口会由对端自动发现（默认 RPC 1995 / gateway 2016），
 * 因此只做“去掉显式端口 + 拒绝明显手误”的处理。
 */
export function parseRemoteHost(
  input: string,
  mode: RemoteMode = "direct",
): { host: string; port?: number } | { error: "port" } | null {
  let text = input.trim();
  if (!text) return null;
  text = text.replace(/^tcp:\/\//i, "").replace(/\/.*$/, "");
  const v6 = text.match(/^\[([^\]]+)\](?::(\d+))?$/);
  if (v6) {
    if (!v6[2]) return { host: v6[1] };
    const port = Number(v6[2]);
    if (mode === "gateway") {
      return port > 0 && port <= 65535 ? { host: v6[1], port } : { error: "port" };
    }
    if (!ACCEPTED_PORTS.has(port)) return { error: "port" };
    return { host: v6[1] };
  }
  const parts = text.split(":");
  if (parts.length === 1) {
    return { host: parts[0] };
  }
  const port = Number(parts[parts.length - 1]);
  if (mode === "gateway") {
    if (!Number.isFinite(port) || port <= 0 || port > 65535) return { error: "port" };
    return { host: parts.slice(0, -1).join(":"), port };
  }
  if (!ACCEPTED_PORTS.has(port)) {
    return { error: "port" };
  }
  return { host: parts.slice(0, -1).join(":") };
}
