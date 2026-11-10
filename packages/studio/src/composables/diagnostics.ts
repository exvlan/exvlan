// 诊断包需要的前端状态（上传与分享共用同一份，避免两边内容不一致）。
//
// 内容：
//   --- local-storage ---   app_mode / lang / log_level / last_network_instance_id
//   --- vpn ---             移动端 VpnService 诊断（桌面为空）
//   --- config (N) ---      每个实例的 networking_method / peer_urls / dhcp /
//                           virtual_ipv4 / no_tun / enable_magic_dns / routes / proxy_cidrs

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

  const localKeys = ["app_mode", "lang", "log_level", "last_network_instance_id"];
  const local = ["--- local-storage ---"];
  for (const key of localKeys) {
    local.push(`  ${key}: ${localStorage.getItem(key) ?? "<null>"}`);
  }

  const lines = [local.join("\n"), ...(isMobile ? [mobileVpnDiagnostics()] : [])];
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
