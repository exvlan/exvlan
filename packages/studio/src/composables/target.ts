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

export interface RemoteTarget {
  id: string;
  /** 显示名（缺省用 host:port） */
  name: string;
  host: string;
  port: number;
}

export const LOCAL_TARGET = "local";
export const DEFAULT_RPC_PORT = 15999;

const KEY_RUNTIME = "runtime";
const KEY_TARGET = "target";
const KEY_REMOTES = "remotes";

export function defaultRuntime(): RuntimeKind {
  // 7.3 起默认服务模式：开机自启、关界面继续跑。
  return "service";
}

/** 运行方式；缺失时从旧的 `app_mode` 迁移。 */
export function loadRuntime(): RuntimeKind {
  const raw = localStorage.getItem(KEY_RUNTIME);
  if (raw === "service" || raw === "normal" || raw === "off") {
    return raw;
  }
  const legacy = migrateLegacyMode();
  if (legacy) {
    return legacy.runtime;
  }
  return defaultRuntime();
}

export async function saveRuntime(runtime: RuntimeKind): Promise<void> {
  await persistUiStateNow(KEY_RUNTIME, runtime);
}

export function loadRemotes(): RemoteTarget[] {
  const raw = localStorage.getItem(KEY_REMOTES);
  if (!raw) {
    const legacy = migrateLegacyMode();
    return legacy?.remotes ?? [];
  }
  try {
    const list = JSON.parse(raw);
    if (!Array.isArray(list)) return [];
    return list.filter((item): item is RemoteTarget => {
      return (
        item && typeof item.id === "string" && typeof item.host === "string" && item.host.length > 0
      );
    });
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
  const legacy = migrateLegacyMode();
  if (legacy?.target) {
    return legacy.target;
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
  return remote.name || `${remote.host}:${remote.port || DEFAULT_RPC_PORT}`;
}

export function newRemote(host = "", port = DEFAULT_RPC_PORT): RemoteTarget {
  return {
    id: `rm-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`,
    name: "",
    host,
    port,
  };
}

/** 解析 `host:port` / `tcp://host:port` / 纯 host。 */
export function parseAddress(input: string): { host: string; port: number } | null {
  let text = input.trim();
  if (!text) return null;
  text = text.replace(/^tcp:\/\//i, "").replace(/\/.*$/, "");
  // [::1]:15999
  const v6 = text.match(/^\[([^\]]+)\](?::(\d+))?$/);
  if (v6) {
    return { host: v6[1], port: v6[2] ? Number(v6[2]) : DEFAULT_RPC_PORT };
  }
  const parts = text.split(":");
  if (parts.length === 1) {
    return { host: parts[0], port: DEFAULT_RPC_PORT };
  }
  const port = Number(parts[parts.length - 1]);
  if (!Number.isFinite(port) || port <= 0 || port > 65535) {
    return null;
  }
  return { host: parts.slice(0, -1).join(":"), port };
}

interface LegacyMode {
  mode?: string;
  remote_rpc_address?: string;
}

/** 7.2 → 7.3：`app_mode` 拆成 runtime + target。 */
function migrateLegacyMode(): {
  runtime: RuntimeKind;
  target?: string;
  remotes?: RemoteTarget[];
} | null {
  const raw = localStorage.getItem("app_mode");
  if (!raw) return null;
  let mode: LegacyMode;
  try {
    mode = JSON.parse(raw) as LegacyMode;
  } catch {
    return null;
  }
  if (mode.mode === "service") {
    return { runtime: "service" };
  }
  if (mode.mode === "normal") {
    return { runtime: "normal" };
  }
  if (mode.mode === "remote") {
    const addr = parseAddress(mode.remote_rpc_address ?? "");
    if (!addr) {
      // 地址无效：保留「本机服务模式」，不让用户启动后无处可连。
      return { runtime: "service" };
    }
    const remote: RemoteTarget = {
      id: `rm-legacy-${addr.host}-${addr.port}`,
      name: `${addr.host}:${addr.port}`,
      host: addr.host,
      port: addr.port,
    };
    return { runtime: "service", target: remote.id, remotes: [remote] };
  }
  return null;
}
