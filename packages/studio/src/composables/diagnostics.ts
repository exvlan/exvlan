// 诊断包需要的前端状态（上传与分享共用同一份，避免两边内容不一致）。
//
// 内容：
//   --- local-storage ---   app_mode / lang / log_level / last_network_instance_id
//   --- vpn ---             移动端 VpnService 诊断（桌面为空）
//   --- config (N) ---      每个实例的 networking_method / peer_urls / dhcp /
//                           virtual_ipv4 / no_tun / enable_magic_dns / routes / proxy_cidrs

import { invoke } from "@tauri-apps/api/core";
import { type } from "@tauri-apps/plugin-os";
import { Utils } from "@exvlan/std";
import { getConfig, listNetworkInstanceIds } from "~/composables/backend";
import { mobileVpnDiagnostics } from "~/composables/mobile_vpn";

export function withTimeout<T>(p: Promise<T>, ms: number, label: string): Promise<T> {
  return Promise.race([
    p,
    new Promise<T>((_, reject) =>
      setTimeout(() => reject(new Error(`${label} timeout after ${ms}ms`)), ms),
    ),
  ]);
}

export async function buildFrontendState(): Promise<string> {
  const platform = type();
  const isMobile = platform === "android" || platform === "ios";

  const lines: string[] = [];

  // WebView 存储是否真的可用（macOS 上曾因 WebKit 目录属主是 root 而整个失效，
  // 表现为「保存了但读不回来」，而旧诊断只列 4 个固定 key，完全看不出来）。
  lines.push(storageSection());

  // 后端 prefs（`get_ui_state`）全量：与 localStorage 对照，能立刻看出「哪边丢了」。
  try {
    const state = await withTimeout(
      invoke<Record<string, string>>("get_ui_state"),
      3000,
      "get_ui_state",
    );
    const prefs = ["--- prefs (backend) ---"];
    for (const [k, v] of Object.entries(state ?? {}).sort(([a], [b]) => a.localeCompare(b))) {
      prefs.push(`  ${k}: ${clip(v)}`);
    }
    lines.push(prefs.join("\n"));
  } catch (e) {
    lines.push(`--- prefs (backend) ---\n  collect failed: ${String(e)}`);
  }

  if (isMobile) {
    lines.push(mobileVpnDiagnostics());
  }
  try {
    const ids = await withTimeout(listNetworkInstanceIds(), 5000, "listNetworkInstanceIds");
    const list = [...(ids.running_inst_ids ?? []), ...(ids.disabled_inst_ids ?? [])]
      .map((id) => (typeof id === "string" ? id : Utils.UuidToStr(id as any)))
      .filter((id) => id.length > 0);
    lines.push(`--- config (${list.length}) ---`);
    for (const id of list) {
      const cfg = await withTimeout(getConfig(id), 3000, `getConfig ${id}`).catch(() => null);
      if (!cfg) {
        lines.push(`instance: ${id} (config unavailable)`);
        continue;
      }
      lines.push(`instance: ${id}`);
      lines.push(`  networking_method: ${cfg.networking_method}`);
      lines.push(`  public_server_url: ${cfg.public_server_url ?? "-"}`);
      lines.push(`  peer_urls: [${(cfg.peer_urls ?? []).join(", ")}]`);
      lines.push(
        `  dhcp: ${cfg.dhcp} virtual_ipv4: ${cfg.virtual_ipv4 || "-"}/${cfg.network_length ?? "-"}`,
      );
      lines.push(`  no_tun: ${cfg.no_tun} enable_magic_dns: ${cfg.enable_magic_dns}`);
      lines.push(`  routes: [${(cfg.routes ?? []).join(", ")}]`);
      lines.push(`  proxy_cidrs: [${(cfg.proxy_cidrs ?? []).join(", ")}]`);
    }
  } catch (e: any) {
    lines.push(`--- config ---\ncollect failed: ${String(e)}`);
  }
  return lines.join("\n");
}

/** 截断过长值（保住诊断包可读性）。 */
function clip(value: string | null | undefined, max = 200): string {
  if (value === null || value === undefined) return "<null>";
  return value.length > max ? `${value.slice(0, max)}…(${value.length}B)` : value;
}

/**
 * storage 探针 + 全量 dump。
 *
 * 只看固定 key 的老诊断在 macOS 上完全看不出「localStorage 是黑洞」——
 * 这里做一次写→读→删，并把所有 key 都列出来。
 */
function storageSection(): string {
  const out = ["--- storage ---"];
  const probeKey = "__diag_probe__";
  const value = `p${Date.now()}`;
  let probe = "ok";
  try {
    localStorage.setItem(probeKey, value);
    const back = localStorage.getItem(probeKey);
    if (back !== value) {
      probe = `write-then-read mismatch (wrote=${clip(value, 20)}, read=${clip(back, 20)})`;
    }
    localStorage.removeItem(probeKey);
  } catch (e) {
    probe = `unavailable: ${String(e)}`;
  }
  out.push(`  probe: ${probe}`);
  out.push(`  length: ${localStorage.length}`);
  const keys: string[] = [];
  for (let i = 0; i < localStorage.length; i += 1) {
    const key = localStorage.key(i);
    if (key) keys.push(key);
  }
  for (const key of keys.sort()) {
    out.push(`  ${key}: ${clip(localStorage.getItem(key) ?? "<null>")}`);
  }
  return out.join("\n");
}
