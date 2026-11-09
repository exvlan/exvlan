import type { NetworkTypes } from "@exvlan/std";
import { type as osType } from "@tauri-apps/plugin-os";
import { addPluginListener } from "@tauri-apps/api/core";
import { Utils } from "@exvlan/std";
import {
  consume_vpn_tile_action,
  get_vpn_status,
  prepare_vpn,
  start_vpn,
  stop_vpn,
  type VpnTileAction,
} from "tauri-plugin-vpnservice-api";
import {
  collectNetworkInfo,
  getConfig,
  listNetworkInstanceIds,
  logDiagStep,
  logVpnRoutes,
  setTunFd,
} from "./backend";

type Route = NetworkTypes.Route;

interface vpnStatus {
  running: boolean;
  ipv4Addr: string | null | undefined;
  ipv4Cidr: number | null | undefined;
  routes: string[];
  dns: string | null | undefined;
}

let vpnReconcileTimer: ReturnType<typeof setTimeout> | null = null;
const VPN_RECONCILE_INTERVAL_MS = 2000;
const VPN_RECONCILE_MAX_ATTEMPTS = 60;

let desiredVpnInstanceId: string | undefined;
let activeVpnInstanceId: string | undefined;
let vpnReconcileGeneration = 0;
let vpnReconcileAttempts = 0;
let vpnReconcileQueue: Promise<void> = Promise.resolve();
let vpnPermissionRequest: Promise<boolean> | null = null;
let vpnTileActionHandler: ((action: VpnTileAction) => Promise<void>) | undefined;
let vpnTileActionQueue: Promise<void> = Promise.resolve();

/** 供 UI 直接绑定的 VPN 运行态（实例在跑但 VPN 没起来 = 需要提示）。 */
export const mobileVpnRunning = ref(false);
export const mobileVpnSyncing = ref(false);
/** 上次启动是否因为用户拒绝 VPN 授权而失败（横幅文案据此切换）。 */
export const mobileVpnPermissionDenied = ref(false);

const curVpnStatus: vpnStatus = {
  running: false,
  ipv4Addr: undefined,
  ipv4Cidr: undefined,
  routes: [],
  dns: undefined,
};

export function setMobileVpnTileActionHandler(handler?: (action: VpnTileAction) => Promise<void>) {
  vpnTileActionHandler = handler;
}

export async function consumePendingMobileVpnTileAction() {
  const handler = vpnTileActionHandler;
  if (!handler) {
    return false;
  }

  const action = (await consume_vpn_tile_action())?.action;
  if (action !== "start" && action !== "stop") {
    return false;
  }

  const run = vpnTileActionQueue
    .catch((error) => console.error("previous VPN tile action failed", error))
    .then(() => handler(action));
  vpnTileActionQueue = run.catch((error) => console.error("VPN tile action failed", error));
  await run;
  return true;
}

async function requestVpnPermissionOnce() {
  console.log("prepare vpn");
  const prepare_ret = await prepare_vpn();
  console.log("prepare vpn", JSON.stringify(prepare_ret));
  if (prepare_ret?.errorMsg?.length) {
    throw new Error(prepare_ret.errorMsg);
  }

  const granted = prepare_ret?.granted ?? true;
  if (!granted) {
    console.info("vpn permission request was denied or dismissed");
  }

  return granted;
}

async function requestVpnPermission() {
  if (vpnPermissionRequest) {
    console.log("reuse pending vpn permission request");
    return await vpnPermissionRequest;
  }

  const request = requestVpnPermissionOnce();
  vpnPermissionRequest = request;
  try {
    return await request;
  } finally {
    if (vpnPermissionRequest === request) {
      vpnPermissionRequest = null;
    }
  }
}

function clearVpnReconcileTimer() {
  if (vpnReconcileTimer) {
    clearTimeout(vpnReconcileTimer);
    vpnReconcileTimer = null;
  }
}

function beginVpnReconcile(instanceId?: string) {
  clearVpnReconcileTimer();
  desiredVpnInstanceId = instanceId;
  vpnReconcileAttempts = 0;
  vpnReconcileGeneration += 1;
  return vpnReconcileGeneration;
}

function isCurrentVpnReconcile(instanceId: string, generation: number) {
  return (
    desiredVpnInstanceId === (instanceId || undefined) && vpnReconcileGeneration === generation
  );
}

function scheduleVpnReconcile(instanceId: string, generation: number, reason: string) {
  if (!isCurrentVpnReconcile(instanceId, generation)) return;

  if (vpnReconcileAttempts >= VPN_RECONCILE_MAX_ATTEMPTS) {
    console.error(
      "vpn service reconcile stopped after maximum attempts",
      instanceId,
      VPN_RECONCILE_MAX_ATTEMPTS,
      reason,
    );
    return;
  }

  clearVpnReconcileTimer();
  vpnReconcileAttempts += 1;
  console.log(
    "vpn service is not ready, retrying",
    JSON.stringify({
      instanceId,
      attempt: vpnReconcileAttempts,
      maxAttempts: VPN_RECONCILE_MAX_ATTEMPTS,
      delayMs: VPN_RECONCILE_INTERVAL_MS,
      reason,
    }),
  );
  vpnReconcileTimer = setTimeout(() => {
    vpnReconcileTimer = null;
    void enqueueVpnReconcile(instanceId, generation);
  }, VPN_RECONCILE_INTERVAL_MS);
}

function resetVpnConfigStatus() {
  curVpnStatus.ipv4Addr = undefined;
  curVpnStatus.ipv4Cidr = undefined;
  curVpnStatus.routes = [];
  curVpnStatus.dns = undefined;
}

function syncVpnStatusFromNative(status: Awaited<ReturnType<typeof get_vpn_status>>) {
  curVpnStatus.running = status?.running ?? false;
  mobileVpnRunning.value = curVpnStatus.running;
  if (!curVpnStatus.running) {
    activeVpnInstanceId = undefined;
    resetVpnConfigStatus();
    return;
  }

  const ipv4WithCidr = status?.ipv4Addr;
  if (ipv4WithCidr?.length) {
    const [ipv4Addr, cidr] = ipv4WithCidr.split("/");
    curVpnStatus.ipv4Addr = ipv4Addr;

    const parsedCidr = Number(cidr);
    curVpnStatus.ipv4Cidr = Number.isInteger(parsedCidr) ? parsedCidr : undefined;
  } else {
    curVpnStatus.ipv4Addr = undefined;
    curVpnStatus.ipv4Cidr = undefined;
  }

  curVpnStatus.routes = [...(status?.routes ?? [])];
  curVpnStatus.dns = status?.dns ?? undefined;
}

/** 供网络自检使用：VPN 运行时状态（只有前端知道）。 */
export function mobileVpnDiagnostics(): string {
  const lines = [
    "--- vpn ---",
    `desired_instance: ${desiredVpnInstanceId ?? "-"}`,
    `active_instance: ${activeVpnInstanceId ?? "-"}`,
    `running: ${curVpnStatus.running}`,
    `ipv4: ${curVpnStatus.ipv4Addr ?? "-"}/${curVpnStatus.ipv4Cidr ?? "-"}`,
    `dns: ${curVpnStatus.dns ?? "-"}`,
    `routes: ${curVpnStatus.routes.length}`,
    ...curVpnStatus.routes.map((r) => `  ${r}`),
  ];
  return lines.join("\n");
}

async function waitVpnStatus(target_status: boolean, timeout_sec: number) {
  const start_time = Date.now();
  while (curVpnStatus.running !== target_status) {
    if (Date.now() - start_time > timeout_sec * 1000) {
      throw new Error("wait vpn status timeout");
    }
    await new Promise((r) => setTimeout(r, 50));
  }
}

async function doStopVpn(force = false) {
  const wasRunning = curVpnStatus.running;
  if (!force && !wasRunning) {
    activeVpnInstanceId = undefined;
    return;
  }
  console.log("stop vpn");
  const stop_ret = await stop_vpn();
  console.log("stop vpn", JSON.stringify(stop_ret));
  if (wasRunning) {
    await waitVpnStatus(false, 3);
  }

  activeVpnInstanceId = undefined;
  resetVpnConfigStatus();
  mobileVpnRunning.value = false;
}

async function doStartVpn(
  instanceId: string,
  ipv4Addr: string,
  cidr: number,
  routes: string[],
  dns?: string,
) {
  if (curVpnStatus.running) {
    return;
  }

  console.log("start vpn service", ipv4Addr, cidr, routes, dns);
  const request = {
    ipv4Addr: `${ipv4Addr}/${cidr}`,
    routes,
    dns,
    disallowedApplications: ["era.exvlan.studio"],
    mtu: 1300,
  };

  mobileVpnSyncing.value = true;
  try {
    let start_ret = await start_vpn(request);
    console.log("start vpn response", JSON.stringify(start_ret));
    if (start_ret?.errorMsg === "need_prepare") {
      const granted = await requestVpnPermission();
      if (!granted) {
        mobileVpnPermissionDenied.value = true;
        throw new Error("vpn_permission_denied");
      }
      mobileVpnPermissionDenied.value = false;
      start_ret = await start_vpn(request);
      console.log("start vpn retry response", JSON.stringify(start_ret));
    }

    if (start_ret?.errorMsg?.length) {
      throw new Error(start_ret.errorMsg);
    }
    await waitVpnStatus(true, 3);

    curVpnStatus.ipv4Addr = ipv4Addr;
    curVpnStatus.ipv4Cidr = cidr;
    curVpnStatus.routes = routes;
    curVpnStatus.dns = dns;
    activeVpnInstanceId = instanceId;
    mobileVpnRunning.value = true;
  } finally {
    mobileVpnSyncing.value = false;
  }
}

async function onVpnServiceStart(payload: any) {
  console.log("vpn service start", JSON.stringify(payload));
  curVpnStatus.running = true;
  mobileVpnRunning.value = true;
  if (payload.fd) {
    await setTunFd(payload.fd).catch((e) => {
      console.error("set tun fd failed", e);
    });
  }
}

async function onVpnServiceStop(payload: any) {
  console.log("vpn service stop", JSON.stringify(payload));
  curVpnStatus.running = false;
  mobileVpnRunning.value = false;
  activeVpnInstanceId = undefined;
  resetVpnConfigStatus();
}

async function registerVpnServiceListener() {
  console.log("register vpn service listener");
  await addPluginListener("vpnservice", "vpn_service_start", onVpnServiceStart);

  await addPluginListener("vpnservice", "vpn_service_stop", onVpnServiceStop);

  await addPluginListener("vpnservice", "vpn_tile_action", () => {
    void consumePendingMobileVpnTileAction().catch((error) => {
      console.error("consume VPN tile action failed", error);
    });
  });
}

function ipv4ToInt(ip: string): number | null {
  const parts = ip.split(".");
  if (parts.length !== 4) {
    return null;
  }
  let value = 0;
  for (const part of parts) {
    const n = Number.parseInt(part, 10);
    if (!Number.isInteger(n) || n < 0 || n > 255) {
      return null;
    }
    value = (value << 8) | n;
  }
  return value >>> 0;
}

function intToIpv4(value: number): string {
  return [24, 16, 8, 0].map((shift) => (value >>> shift) & 0xff).join(".");
}

/** 规范化成 network/prefix；非法返回 null（宁可少一条，也不让 VpnService.Builder 抛异常）。 */
function normalizeCidr(value: unknown, fallbackPrefix = 32): string | null {
  const raw = String(value ?? "").trim();
  if (!raw) {
    return null;
  }
  const slash = raw.indexOf("/");
  const addr = (slash >= 0 ? raw.slice(0, slash) : raw).trim();
  if (!addr) {
    return null;
  }
  const isV6 = addr.includes(":");
  const prefixRaw = slash >= 0 ? raw.slice(slash + 1).trim() : "";
  const prefix = prefixRaw === "" ? (isV6 ? 128 : fallbackPrefix) : Number.parseInt(prefixRaw, 10);
  if (!Number.isInteger(prefix) || prefix < 0 || prefix > (isV6 ? 128 : 32)) {
    return null;
  }
  if (isV6) {
    // IPv6 交给 Kotlin 侧解析，这里只做最小校验
    return `${addr}/${prefix}`;
  }
  const n = ipv4ToInt(addr);
  if (n === null) {
    return null;
  }
  const mask = prefix === 0 ? 0 : (0xffffffff << (32 - prefix)) >>> 0;
  return `${intToIpv4((n & mask) >>> 0)}/${prefix}`;
}

/**
 * 计算要下发给 Android VpnService 的路由。
 *
 * 历史 bug：只取 `Route.proxy_cidrs`，完全忽略 `Route.ipv4_addr`，也没有本机虚拟网段，
 * 于是默认配置（proxy_cidrs 为空）下只会下发 magic DNS 的 /32，
 * 手机根本不把 peer 流量送进隧道 → 表现为「组网失败」。
 */
export function getRoutesForVpn(
  routes: Route[] | undefined,
  node_config: NetworkTypes.NetworkConfig,
  selfIp?: string | null,
  selfPrefix?: number,
): string[] {
  const out = new Set<string>();
  const skipped: string[] = [];

  const push = (value: unknown, fallbackPrefix = 32) => {
    const cidr = normalizeCidr(value, fallbackPrefix);
    if (cidr) {
      out.add(cidr);
    } else if (value !== undefined && value !== null && String(value).trim() !== "") {
      skipped.push(String(value));
    }
  };

  // 1) 本机虚拟网段（10.126.126.224/24 → 10.126.126.0/24）
  if (selfIp) {
    push(`${selfIp}/${selfPrefix ?? 32}`, selfPrefix ?? 32);
  }

  // 2) 每个 peer：自身虚拟 IP（优先用自带前缀）+ 它代理的网段
  for (const r of routes ?? []) {
    const addr = r.ipv4_addr;
    if (typeof addr === "string") {
      push(addr, 32);
    } else if (addr && "address" in addr && addr.address) {
      const prefix = addr.network_length ?? 32;
      push(`${Utils.ipv4ToString(addr.address)}/${prefix}`, prefix);
    }
    for (const cidr of r.proxy_cidrs ?? []) {
      push(cidr);
    }
    // IPv6 peer（若有）：同样优先用自带前缀，缺省 /128
    const v6 = r.ipv6_addr;
    if (v6?.address) {
      const prefix = v6.network_length ?? 128;
      push(`${Utils.ipv6ToString(v6.address)}/${prefix}`, prefix);
    }
  }

  // 3) 用户自定义 routes
  for (const route of node_config.routes ?? []) {
    push(route);
  }

  // 4) magic DNS
  if (node_config.enable_magic_dns) {
    push("100.100.100.101/32");
  }

  if (skipped.length) {
    console.warn("[vpn] skipped invalid routes", skipped);
    void logVpnRoutes([`skipped:${skipped.join(",")}`]).catch(() => {});
  }

  return Array.from(out).sort();
}

async function stopVpnOwnedByOtherInstance(instanceId: string, generation: number) {
  if (!isCurrentVpnReconcile(instanceId, generation)) return false;

  if (curVpnStatus.running && activeVpnInstanceId !== instanceId) {
    console.warn("vpn service owner changed", activeVpnInstanceId, instanceId);
    await doStopVpn();
  }

  return isCurrentVpnReconcile(instanceId, generation);
}

async function reconcileNetworkInstance(instanceId: string, generation: number) {
  if (!isCurrentVpnReconcile(instanceId, generation)) return;

  clearVpnReconcileTimer();

  if (!instanceId) {
    console.warn("vpn service skipped because instance id is empty");
    if (curVpnStatus.running) {
      await doStopVpn();
    }
    return;
  }
  const config = await getConfig(instanceId);
  if (!isCurrentVpnReconcile(instanceId, generation)) return;

  console.log(
    "vpn service loaded config",
    instanceId,
    JSON.stringify({
      no_tun: config.no_tun,
      dhcp: config.dhcp,
      enable_magic_dns: config.enable_magic_dns,
    }),
  );
  if (config.no_tun) {
    console.log("vpn service skipped because no_tun is enabled", instanceId);
    if (activeVpnInstanceId === instanceId) {
      await doStopVpn();
    }
    return;
  }

  if (!(await stopVpnOwnedByOtherInstance(instanceId, generation))) return;

  let curNetworkInfo;
  try {
    curNetworkInfo = (await collectNetworkInfo(instanceId))?.info?.map?.[instanceId];
  } catch (e) {
    console.warn("vpn service network info query failed", instanceId, e);
    scheduleVpnReconcile(instanceId, generation, "network_info_query_failed");
    return;
  }

  if (!isCurrentVpnReconcile(instanceId, generation)) return;

  if (!curNetworkInfo) {
    scheduleVpnReconcile(instanceId, generation, "network_info_unavailable");
    return;
  }

  if (curNetworkInfo.error_msg?.length) {
    console.warn(
      "vpn service skipped because network instance failed",
      instanceId,
      curNetworkInfo.error_msg,
    );
    vpnReconcileAttempts = 0;
    await doStopVpn();
    return;
  }

  const virtualIpv4 = curNetworkInfo.my_node_info?.virtual_ipv4;
  const virtual_ip = virtualIpv4?.address?.addr
    ? Utils.ipv4ToString(virtualIpv4.address)
    : undefined;

  if (!virtual_ip || !virtual_ip.length) {
    scheduleVpnReconcile(
      instanceId,
      generation,
      config.dhcp ? "dhcp_ipv4_unavailable" : "static_ipv4_unavailable",
    );
    return;
  }

  vpnReconcileAttempts = 0;

  let network_length = virtualIpv4?.network_length;
  if (!network_length) {
    network_length = 24;
  }

  const routes = getRoutesForVpn(curNetworkInfo?.routes, config, virtual_ip, network_length);

  const dns = config.enable_magic_dns ? "100.100.100.101" : undefined;

  const ipChanged = virtual_ip !== curVpnStatus.ipv4Addr;
  const cidrChanged = network_length !== curVpnStatus.ipv4Cidr;
  const routesChanged = JSON.stringify(routes) !== JSON.stringify(curVpnStatus.routes);
  const dnsChanged = dns != curVpnStatus.dns;
  const configChanged = ipChanged || cidrChanged || routesChanged || dnsChanged;
  const shouldStartVpn = !curVpnStatus.running;

  if (shouldStartVpn || configChanged) {
    console.info("vpn service virtual ip changed", JSON.stringify(curVpnStatus), virtual_ip);
    if (curVpnStatus.running) {
      try {
        await doStopVpn();
      } catch (e) {
        console.error(e);
      }
    }

    try {
      if (!isCurrentVpnReconcile(instanceId, generation)) return;

      void logVpnRoutes(routes).catch(() => {});
      await doStartVpn(instanceId, virtual_ip, network_length, routes, dns);
      if (!isCurrentVpnReconcile(instanceId, generation) && activeVpnInstanceId === instanceId) {
        await doStopVpn();
      }
    } catch (e) {
      if (e instanceof Error && e.message === "need_prepare") {
        console.info("vpn permission is required before starting the Android VPN service");
        return;
      }
      if (e instanceof Error && e.message === "vpn_permission_denied") {
        console.info("vpn permission request was denied or dismissed");
        return;
      }
      console.error("start vpn service failed", e);
    }
  }
}

function enqueueVpnTask(task: () => Promise<void>) {
  const run = vpnReconcileQueue
    .catch((e) => {
      console.error("previous vpn service reconcile failed", e);
    })
    .then(task);
  vpnReconcileQueue = run.catch((e) => {
    console.error("vpn service reconcile failed", e);
  });
  return run;
}

function enqueueVpnReconcile(instanceId: string, generation: number) {
  return enqueueVpnTask(() => reconcileNetworkInstance(instanceId, generation));
}

export async function onNetworkInstanceChange(instanceId: string) {
  const generation = beginVpnReconcile(instanceId || undefined);

  if (instanceId && (await isNoTunEnabled(instanceId))) {
    if (vpnReconcileGeneration !== generation) return;

    if (activeVpnInstanceId === instanceId) {
      desiredVpnInstanceId = undefined;
      await enqueueVpnReconcile("", generation);
      return;
    }

    desiredVpnInstanceId = activeVpnInstanceId;
    if (activeVpnInstanceId) {
      await enqueueVpnReconcile(activeVpnInstanceId, generation);
    }
    return;
  }

  if (vpnReconcileGeneration !== generation) return;

  await enqueueVpnReconcile(instanceId, generation);
}

export async function onNetworkInstanceUpdate(instanceId: string) {
  if (!instanceId || instanceId !== desiredVpnInstanceId) return;

  const generation = beginVpnReconcile(instanceId);
  await enqueueVpnReconcile(instanceId, generation);
}

async function isNoTunEnabled(instanceId: string | undefined) {
  if (!instanceId) {
    return false;
  }
  return (await getConfig(instanceId)).no_tun ?? false;
}

async function findRunningTunInstanceId() {
  const instanceIds = await listNetworkInstanceIds();
  const runningIds = (instanceIds.running_inst_ids ?? []).map(Utils.UuidToStr);
  console.log("vpn service sync running instances", JSON.stringify(runningIds));

  for (const instanceId of runningIds) {
    if (await isNoTunEnabled(instanceId)) {
      continue;
    }

    return instanceId;
  }

  return undefined;
}

export async function initMobileVpnService() {
  await registerVpnServiceListener();
}

export async function prepareVpnService(instanceId: string) {
  if (await isNoTunEnabled(instanceId)) {
    return;
  }

  const generation = beginVpnReconcile(instanceId);
  const stopPreviousOwner = enqueueVpnTask(async () => {
    await stopVpnOwnedByOtherInstance(instanceId, generation);
  });
  await Promise.all([requestVpnPermission(), stopPreviousOwner]);
}

export async function syncMobileVpnService() {
  // 桌面端没有 VpnService：这里直接返回，避免调用 plugin:vpnservice 触发 ACL 拒绝
  // （历史日志里的 "Command plugin:vpnservice|get_vpn_status not allowed by ACL"）。
  const platform = osType();
  if (platform !== "android" && platform !== "ios") {
    return;
  }
  syncVpnStatusFromNative(await get_vpn_status());
  const instanceId = await findRunningTunInstanceId();
  if (instanceId) {
    console.log("vpn service sync selected instance", instanceId);
    await onNetworkInstanceChange(instanceId);
    return;
  }

  await onNetworkInstanceChange("");
}

/* ---------------- 实例起来后自动接管流量 ---------------- */

const AUTO_SYNC_DELAYS_MS = [1000, 3000, 9000];
let autoSyncTimer: ReturnType<typeof setTimeout> | null = null;
let autoSyncAttempts = 0;

/**
 * 实例在跑但 VpnService 没接管时，按 1s / 3s / 9s 退避补启动。
 *
 * 历史 bug：`syncMobileVpnService()` 只在挂载时调用一次，实例晚于它 ready
 * （冷启动、内核重启、peer 建连慢）就永远停在「运行中但未接管流量」。
 */
export function scheduleVpnAutoSync(reason = "instances-changed") {
  if (mobileVpnRunning.value || mobileVpnSyncing.value) {
    return;
  }
  if (autoSyncTimer || autoSyncAttempts >= AUTO_SYNC_DELAYS_MS.length) {
    return;
  }
  const delay = AUTO_SYNC_DELAYS_MS[autoSyncAttempts];
  autoSyncTimer = setTimeout(async () => {
    autoSyncTimer = null;
    autoSyncAttempts += 1;
    try {
      await syncMobileVpnService();
    } catch (e) {
      console.warn("vpn auto sync failed", e);
    }
    logDiagStep(
      "vpn_auto_sync",
      `attempt=${autoSyncAttempts} running=${mobileVpnRunning.value} reason=${reason}`,
    );
    if (mobileVpnRunning.value) {
      autoSyncAttempts = 0;
    } else {
      scheduleVpnAutoSync(reason);
    }
  }, delay);
}

/** 用户主动断开/重新连上网络时重置退避计数。 */
export function resetVpnAutoSync() {
  if (autoSyncTimer) {
    clearTimeout(autoSyncTimer);
    autoSyncTimer = null;
  }
  autoSyncAttempts = 0;
}
