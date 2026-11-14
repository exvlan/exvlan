import { Event, listen } from "@tauri-apps/api/event";
import { type } from "@tauri-apps/plugin-os";
import { NetworkTypes } from "@exvlan/std";
import { Utils } from "@exvlan/std";
import { normalizeConfigSource } from "./config_source";
import { reportFrontendError, setNetworks } from "./backend";
import { onNetworkInstanceUpdate } from "./mobile_vpn";

interface StoredGuiConfig {
  config: NetworkTypes.NetworkConfig;
  source?: unknown;
}

const EVENTS = Object.freeze({
  SAVE_CONFIGS: "save_configs",
  PRE_RUN_NETWORK_INSTANCE: "pre_run_network_instance",
  POST_RUN_NETWORK_INSTANCE: "post_run_network_instance",
  VPN_SERVICE_STOP: "vpn_service_stop",
  DHCP_IP_CHANGED: "dhcp_ip_changed",
  PROXY_CIDRS_UPDATED: "proxy_cidrs_updated",
  EVENT_LAGGED: "event_lagged",
});

function onSaveConfigs(event: Event<StoredGuiConfig[]>) {
  console.log(`Received event '${EVENTS.SAVE_CONFIGS}': ${event.payload}`);
  const list = event.payload.map(({ config, source }) => ({
    config: NetworkTypes.normalizeNetworkConfig(config),
    source: normalizeConfigSource(source),
  }));
  // 桌面端已经不再发这个事件（配置目录才是权威）；移动端仍用它把内存存储
  // 落到 `networks.toml`。写失败只上报，不回退第二份存储（§14.5）。
  void setNetworks(JSON.stringify(list)).catch((e) => {
    console.warn("setNetworks failed", e);
    void reportFrontendError("fe:networks", `setNetworks failed: ${String(e)}`).catch(() => {});
  });
}

function normalizeInstanceIdPayload(payload: unknown): string {
  if (typeof payload === "string") {
    return payload;
  }

  if (payload && typeof payload === "object") {
    const uuid = payload as Partial<Utils.UUID>;
    if (
      typeof uuid.part1 === "number" &&
      typeof uuid.part2 === "number" &&
      typeof uuid.part3 === "number" &&
      typeof uuid.part4 === "number"
    ) {
      return Utils.UuidToStr(uuid as Utils.UUID);
    }
  }

  if (payload == null) {
    return "";
  }

  const fallback = String(payload);
  return fallback === "[object Object]" ? "" : fallback;
}

async function onPreRunNetworkInstance(event: Event<unknown>) {
  const instanceId = normalizeInstanceIdPayload(event.payload);
  console.log(
    `Received event '${EVENTS.PRE_RUN_NETWORK_INSTANCE}', raw payload:`,
    event.payload,
    "normalized:",
    instanceId,
  );
  if (type() === "android") {
    await prepareVpnService(instanceId);
  }
}

async function onPostRunNetworkInstance(event: Event<unknown>) {
  const instanceId = normalizeInstanceIdPayload(event.payload);
  console.log(
    `Received event '${EVENTS.POST_RUN_NETWORK_INSTANCE}', raw payload:`,
    event.payload,
    "normalized:",
    instanceId,
  );
  if (type() === "android") {
    await onNetworkInstanceChange(instanceId);
  }
}

async function onVpnServiceStop(event: Event<unknown>) {
  console.log(`Received event '${EVENTS.VPN_SERVICE_STOP}', raw payload:`, event.payload);
  // 桌面端也会发这个事件（内核通知「没有 TUN 实例」），但 vpnservice 插件只存在于移动端。
  if (type() !== "android") {
    return;
  }
  await syncMobileVpnService();
}

async function onDhcpIpChanged(event: Event<unknown>) {
  const instanceId = normalizeInstanceIdPayload(event.payload);
  console.log(`Received event '${EVENTS.DHCP_IP_CHANGED}' for instance: ${instanceId}`);
  if (type() === "android") {
    await onNetworkInstanceUpdate(instanceId);
  }
}

async function onProxyCidrsUpdated(event: Event<unknown>) {
  const instanceId = normalizeInstanceIdPayload(event.payload);
  console.log(`Received event '${EVENTS.PROXY_CIDRS_UPDATED}' for instance: ${instanceId}`);
  if (type() === "android") {
    await onNetworkInstanceUpdate(instanceId);
  }
}

async function onEventLagged(event: Event<unknown>) {
  if (type() === "android") {
    await onNetworkInstanceUpdate(normalizeInstanceIdPayload(event.payload));
  }
}

export async function listenGlobalEvents() {
  const unlisteners = [
    await listen(EVENTS.SAVE_CONFIGS, onSaveConfigs),
    await listen(EVENTS.PRE_RUN_NETWORK_INSTANCE, onPreRunNetworkInstance),
    await listen(EVENTS.POST_RUN_NETWORK_INSTANCE, onPostRunNetworkInstance),
    await listen(EVENTS.VPN_SERVICE_STOP, onVpnServiceStop),
    await listen(EVENTS.DHCP_IP_CHANGED, onDhcpIpChanged),
    await listen(EVENTS.PROXY_CIDRS_UPDATED, onProxyCidrsUpdated),
    await listen(EVENTS.EVENT_LAGGED, onEventLagged),
  ];

  return () => {
    unlisteners.forEach((unlisten) => unlisten());
  };
}
