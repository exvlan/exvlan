import { Utils } from "@exvlan/std";
import {
  getConfig,
  listNetworkInstanceIds,
  logDiagStep,
  updateNetworkConfigState,
} from "./backend";
import { loadLastNetworkInstanceId } from "./config";

/**
 * 启动后清理「重复实例」。
 *
 * 多个实例共用同一组监听端口时只有一个能真正启动，其余会以
 * `Address already in use (os error 48)` 失败；坏实例还会拖慢/挂住采集类 RPC。
 *
 * 规则：只在「监听端口集合完全相同」时判定为重复；保留
 * `last_network_instance_id`（不存在或不在候选里则保留第一个），其余置为 disabled。
 * 全程可逆：用户随时可以在列表里重新启用。
 */
export async function dedupeNetworkInstances(): Promise<void> {
  const ids = await listNetworkInstanceIds();
  const running = (ids.running_inst_ids ?? []).map((id) =>
    typeof id === "string" ? id : Utils.UuidToStr(id as any),
  );
  if (running.length < 2) {
    return;
  }

  const preferred = loadLastNetworkInstanceId();
  const groups = new Map<string, string[]>();
  for (const id of running) {
    const cfg = await getConfig(id).catch(() => null);
    if (!cfg) {
      continue;
    }
    const listeners = [...(cfg.listener_urls ?? [])].sort().join(",");
    if (!listeners) {
      continue;
    }
    const list = groups.get(listeners) ?? [];
    list.push(id);
    groups.set(listeners, list);
  }

  for (const [listeners, list] of groups) {
    if (list.length < 2) {
      continue;
    }
    const keep = preferred && list.includes(preferred) ? preferred : list[0];
    for (const id of list) {
      if (id === keep) {
        continue;
      }
      await updateNetworkConfigState(id, true).catch(() => {});
      void logDiagStep("instance_dedupe", `keep=${keep} disable=${id} listeners=${listeners}`);
    }
  }
}
