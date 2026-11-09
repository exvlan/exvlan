import { Utils } from "@exvlan/std";
import {
  collectNetworkInfo,
  getConfig,
  listNetworkInstanceIds,
  logDiagStep,
  updateNetworkConfigState,
} from "./backend";
import { loadLastNetworkInstanceId } from "./config";

/** 给采集 RPC 加超时，避免坏实例把去重流程也拖住。 */
function withTimeout<T>(p: Promise<T>, ms: number): Promise<T> {
  return Promise.race([
    p,
    new Promise<T>((_, reject) => setTimeout(() => reject(new Error("timeout")), ms)),
  ]);
}

/** 该实例是否真的在跑且没有错误（用采集结果判断，而不是只看 id 列表）。 */
async function instanceHealthy(id: string): Promise<boolean> {
  try {
    const resp = await withTimeout(collectNetworkInfo(id), 2500);
    const info = (resp as any)?.info?.map?.[id];
    if (!info) {
      return false;
    }
    const errorMsg = String(info.error_msg ?? "");
    return info.running === true && errorMsg.length === 0;
  } catch {
    return false;
  }
}

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
    // 优先保留「真正能跑起来」的实例：只看 id 列表会把已经崩掉的实例当成正常，
    // 上次就是因此把能用的关掉、留下了报 Address already in use 的那个。
    const healthy: string[] = [];
    for (const id of list) {
      if (await instanceHealthy(id)) {
        healthy.push(id);
      }
    }
    const keep =
      preferred && healthy.includes(preferred)
        ? preferred
        : (healthy[0] ?? (preferred && list.includes(preferred) ? preferred : list[0]));
    for (const id of list) {
      if (id === keep) {
        continue;
      }
      await updateNetworkConfigState(id, true).catch(() => {});
      void logDiagStep("instance_dedupe", `keep=${keep} disable=${id} listeners=${listeners}`);
    }
  }
}
