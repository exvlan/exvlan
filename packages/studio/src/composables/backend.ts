import { invoke } from "@tauri-apps/api/core";
import { Api, NetworkTypes } from "@exvlan/std";
import { type ConfigSource, normalizeConfigSource } from "./config_source";

type NetworkConfig = NetworkTypes.NetworkConfig;
type ValidateConfigResponse = Api.ValidateConfigResponse;
type ListNetworkInstanceIdResponse = Api.ListNetworkInstanceIdResponse;
interface ServiceOptions {
  config_dir: string;
  rpc_portal: string;
  file_log_level: string;
  file_log_dir: string;
  config_server?: string;
}

export type ServiceStatus = "Running" | "Stopped" | "NotInstalled";

interface StoredGuiConfig {
  config: NetworkConfig;
  source: ConfigSource;
}

function parseStoredConfigs(raw: string | null): StoredGuiConfig[] {
  const parsed: unknown = JSON.parse(raw || "[]");
  if (!Array.isArray(parsed)) {
    return [];
  }

  return parsed.flatMap((entry): StoredGuiConfig[] => {
    if (entry && typeof entry === "object" && "config" in entry) {
      const { config, source } = entry as {
        config?: NetworkConfig;
        source?: unknown;
      };
      if (!config) {
        return [];
      }
      return [
        {
          config: NetworkTypes.normalizeNetworkConfig(config),
          source: normalizeConfigSource(source),
        },
      ];
    }

    return [
      {
        config: NetworkTypes.normalizeNetworkConfig(entry as NetworkConfig),
        source: "legacy",
      },
    ];
  });
}

export async function parseNetworkConfig(cfg: NetworkConfig) {
  return invoke<string>("parse_network_config", { cfg: NetworkTypes.toBackendNetworkConfig(cfg) });
}

export async function generateNetworkConfig(tomlConfig: string) {
  const config = await invoke<NetworkConfig>("generate_network_config", { tomlConfig });
  return NetworkTypes.normalizeNetworkConfig(config);
}

export async function runNetworkInstance(cfg: NetworkConfig, save: boolean) {
  return invoke("run_network_instance", { cfg: NetworkTypes.toBackendNetworkConfig(cfg), save });
}

export async function collectNetworkInfo(instanceId: string) {
  return await invoke<Api.CollectNetworkInfoResponse>("collect_network_info", { instanceId });
}

export async function getVpnPortalInfo(instanceId: string) {
  const info = await invoke<NetworkTypes.VpnPortalInfo | undefined>("get_vpn_portal_info", {
    instanceId,
  });
  return info ? NetworkTypes.normalizeVpnPortalInfo(info) : undefined;
}

export async function addVpnPortalClient(
  instanceId: string,
  client: { name: string; virtual_ip: string; groups: string[] },
) {
  return invoke("patch_vpn_portal_clients", {
    instanceId,
    action: "add",
    name: client.name,
    virtualIp: client.virtual_ip,
    groups: client.groups,
  });
}

export async function removeVpnPortalClient(instanceId: string, name: string) {
  return invoke("patch_vpn_portal_clients", { instanceId, action: "remove", name });
}

export async function clearVpnPortalClients(instanceId: string) {
  return invoke("patch_vpn_portal_clients", { instanceId, action: "clear" });
}

export async function setLoggingLevel(level: string) {
  return await invoke("set_logging_level", { level });
}

export async function setTunFd(fd: number) {
  return await invoke("set_tun_fd", { fd });
}

export async function getExvlanVersion() {
  return await invoke<string>("exvlan_version");
}

export async function listNetworkInstanceIds() {
  return await invoke<ListNetworkInstanceIdResponse>("list_network_instance_ids");
}

export async function deleteNetworkInstance(instanceId: string) {
  return await invoke("remove_network_instance", { instanceId });
}

export async function updateNetworkConfigState(instanceId: string, disabled: boolean) {
  return await invoke("update_network_config_state", { instanceId, disabled });
}

export async function saveNetworkConfig(cfg: NetworkConfig) {
  return await invoke("save_network_config", { cfg: NetworkTypes.toBackendNetworkConfig(cfg) });
}

export async function validateConfig(cfg: NetworkConfig) {
  return await invoke<ValidateConfigResponse>("validate_config", {
    cfg: NetworkTypes.toBackendNetworkConfig(cfg),
  });
}

export async function getConfig(instanceId: string) {
  const config = await invoke<NetworkConfig>("get_config", { instanceId });
  return NetworkTypes.normalizeNetworkConfig(config);
}

/** 读取网络列表（`networks.toml` 的 JSON 表示）。 */
export async function getNetworks(): Promise<string> {
  return await invoke<string>("get_networks");
}

/** 写入网络列表（入参是 JSON 字符串）。 */
export async function setNetworks(networks: string) {
  return await invoke("set_networks", { networks });
}

/** 把旧 localStorage 的 networkList 迁移到 networks.toml（只迁移一次）。 */
export async function migrateNetworks(networks: string) {
  return await invoke<boolean>("migrate_networks", { networks });
}

/** 正式版（清理模式）才让前端删除 localStorage 快照。 */
export async function legacyCleanupEnabled(): Promise<boolean> {
  return await invoke<boolean>("legacy_cleanup_enabled");
}

/** 偏好/网络文件路径（诊断面板显示用）。 */
export async function prefsPath() {
  return await invoke<string>("prefs_path_cmd");
}
export async function networksPath() {
  return await invoke<string>("networks_path_cmd");
}

export async function sendConfigs(enabledNetworks: string[]) {
  const fromBackend = await getNetworks().catch(() => "");
  const raw =
    fromBackend && fromBackend !== "[]" ? fromBackend : localStorage.getItem("networkList");
  const networkList = parseStoredConfigs(raw);
  return await invoke("load_configs", {
    configs: networkList.map(({ config, source }) => ({
      config: NetworkTypes.toBackendNetworkConfig(config),
      source,
    })),
    enabledNetworks,
  });
}

export async function getNetworkMetas(instanceIds: string[]) {
  return await invoke<Api.GetNetworkMetasResponse>("get_network_metas", { instanceIds });
}

export async function initService(opts?: ServiceOptions) {
  return await invoke("init_service", { opts });
}

/**
 * 确保「内核已安装 + 服务定义与当前配置一致」。
 *
 * - 指纹一致时后端直接返回 `current`，**不会**弹系统授权框；
 * - 仅在首次安装/配置变化时提权一次。
 */
export async function ensureKernelService(opts: ServiceOptions) {
  return await invoke<"installed" | "current">("ensure_kernel_service", { opts });
}

export async function setServiceStatus(enable: boolean) {
  return await invoke("set_service_status", { enable });
}

export interface KernelUpdateInfo {
  platform: string;
  current?: string;
  latest?: string;
  available: boolean;
  versions: string[];
  source: string;
}

export async function kernelUpdateInfo(source?: string) {
  return await invoke<KernelUpdateInfo>("kernel_update_info", { source });
}

export async function kernelInstallVersion(version: string, source?: string) {
  return await invoke("kernel_install_version", { version, source });
}

export interface AppUpdateInfo {
  platform: string;
  current: string;
  latest?: string;
  notes?: string;
  available: boolean;
  versions: string[];
  source: string;
}

export async function appUpdateInfo(source?: string) {
  return await invoke<AppUpdateInfo>("app_update_info", { source });
}

export async function appDownloadUpdate(version: string, source?: string) {
  return await invoke<string>("app_download_update", { version, source });
}

export async function appInstallUpdate(path: string) {
  return await invoke("app_install_update", { path });
}

/** 安装应用更新：桌面优先官方 updater（成功后自动重启），失败回退「下载 + 打开安装包」。 */
export async function appUpdateApply(version: string, source?: string) {
  return await invoke("app_update_apply", { version, source });
}

export async function getServiceStatus() {
  return await invoke<ServiceStatus>("get_service_status");
}

/** 切换本机运行方式（service / normal / off），不动已建立的连接。 */
export async function runtimeApply(runtime: string) {
  return await invoke<string>("runtime_apply", { runtime });
}

/** 远程目标连通性探测，返回毫秒。只做 TCP 连接，不校验身份。 */
export async function remotePing(url: string) {
  return await invoke<number>("remote_ping", { url });
}

export async function initRpcConnection(isNormalMode: boolean, url?: string) {
  return await invoke("init_rpc_connection", { isNormalMode, url });
}

export async function isClientRunning() {
  return await invoke<boolean>("is_client_running");
}

export async function initWebClient(url?: string) {
  return await invoke("init_web_client", { url });
}

export async function isWebClientConnected() {
  return await invoke<boolean>("is_web_client_connected");
}

/** 前端异常上报：写入 <日志目录>/error.log（scope 建议用 fe:<模块>）。 */
export async function reportFrontendError(scope: string, message: string, stack?: string) {
  try {
    await invoke("report_frontend_error", { scope, message, stack });
  } catch {
    // 上报本身失败时不再递归报错
  }
}

/** error.log 绝对路径。 */
export async function errorLogPath() {
  return await invoke<string>("error_log_path");
}

/** 读取 error.log 尾部（KB）。 */
export async function readErrorLog(tailKb?: number) {
  return await invoke<string>("read_error_log", { tailKb });
}

/** 分享诊断包：Android 弹系统分享面板；桌面返回路径并打开诊断目录。
 *  传入 `frontend` 后与「上传诊断」使用同一份前端状态。 */
export async function shareErrorLog(frontend?: string) {
  return await invoke<string>("share_error_log", { frontend: frontend ?? null });
}

/** 特权服务当前状态（`exvlan-service`；移动端恒为 ready）。 */
export interface ServiceState {
  installed: boolean;
  ready: boolean;
  mode: string;
  kernel_pid: number;
  status: string;
}

export async function serviceState() {
  return await invoke<ServiceState>("service_state");
}

/** 生效的 RPC 白名单：回环 + 各虚拟网段 + 额外追加。 */
export interface WhitelistInfo {
  effective: string;
  applied?: string | null;
  needs_restart?: boolean;
  loopback: string[];
  networks: string[];
  extra: string[];
  notes: string[];
  dhcp_fallback: string;
  max_extra: number;
}

export async function serviceWhitelistInfo() {
  return await invoke<WhitelistInfo>("service_whitelist_info");
}

/** 设置额外允许的网段（后端做 CIDR 校验，拒绝公网段）。 */
export async function serviceExtraWhitelistSet(extra: string[]) {
  return await invoke<WhitelistInfo>("service_extra_whitelist_set", { extra });
}

/** 卸载特权服务（提权），返回卸载后的运行方式（normal/off）。 */
export async function serviceUninstall(runtime: string) {
  return await invoke<string>("service_uninstall", { runtime });
}

export interface PortOwnerInfo {
  port: number;
  free?: boolean;
  pid?: number;
  name?: string;
  path?: string;
  is_our_kernel?: boolean;
}

/** 谁占着 15999（诊断用）。 */
export async function portOwnerInfo() {
  return await invoke<PortOwnerInfo>("port_owner_info");
}

/** 结束占用 15999 的自家内核（后端校验进程路径，非自家拒绝）。 */
export async function portCleanup() {
  return await invoke<void>("port_cleanup");
}

/** 重试安装特权服务（会弹一次系统授权）。 */
export async function serviceInstall() {
  return await invoke<string>("service_install");
}

export interface ServiceDefaultDirs {
  config_dir: string;
  file_log_dir: string;
}

/** 服务模式（root 守护进程）的默认目录：系统路径，避免 root 写用户目录。 */
export async function serviceDefaultDirs() {
  return await invoke<ServiceDefaultDirs>("service_default_dirs_cmd");
}

export interface DiagnosticsInfo {
  version: string;
  os: string;
  arch: string;
  home: string;
  log_dir?: string | null;
  log_dir_writable?: boolean | null;
  config_dir?: string | null;
  config_dir_writable?: boolean | null;
  error_log?: string | null;
  error_log_size?: number | null;
  kernel_installed?: boolean | null;
  kernel_version?: string | null;
  service_status?: string | null;
  service_config_dir?: string | null;
  service_log_dir?: string | null;
}

/** 诊断信息：路径/可写性/版本/内核与服务状态。 */
export async function diagnosticsInfo() {
  return await invoke<DiagnosticsInfo>("diagnostics_info");
}

/** 服务自检（只读）：内核/服务状态、RPC 连通性、daemon 日志尾部。 */
export async function serviceDiagnostics(rpcPortal?: string) {
  return await invoke<string>("service_diagnostics", { rpcPortal });
}

/** 重启服务（提权）。 */
export async function serviceRestart() {
  return await invoke("service_restart");
}

/** 强制重装服务（提权，含目录预建与真连自检）。 */
export async function serviceReinstall(opts: ServiceOptions) {
  return await invoke("service_reinstall", { opts });
}

/** 上报启动时的模式/上次网络（写进 error.log 的 [boot.ui]）。 */
export async function reportBootState(
  mode: string,
  lastInstance?: string | null,
  serviceCurrent?: boolean | null,
  modeSource?: string,
) {
  try {
    await invoke("report_boot_state", { mode, lastInstance, serviceCurrent, modeSource });
  } catch {
    // 诊断上报失败不影响启动
  }
}

/** 上传诊断 zip 到私有仓库（Cloudflare Worker），返回 issue 与 zip 下载链接。 */
export async function uploadDiagnostics(
  frontendState?: string,
  reason: "manual" | "install-failed" = "manual",
) {
  return await invoke<string>("upload_diagnostics", { frontend: frontendState, reason });
}

/** 诊断链路埋点：写 events.log（失败静默）。 */
export async function logDiagStep(step: string, detail?: string) {
  try {
    await invoke("log_diag_step", { step, detail });
  } catch {
    // 埋点失败不影响主流程
  }
}

/** 提权链路自检（Windows 上确认 UAC → 子进程 → 退出码整条链路）。 */
export async function elevateSelftest() {
  return await invoke<string>("elevate_selftest");
}

/** 打开诊断包所在目录（上传失败时的兜底入口）。 */
export async function openDiagnosticsDir() {
  return await invoke<string>("open_diagnostics_dir");
}

/** 网络自检：实例/采集信息 + 最近事件与内核日志。 */
export async function networkDiagnostics(frontendState?: string) {
  return await invoke<string>("network_diagnostics", { frontend: frontendState });
}

/** 把最终下发的 VPN 路由写进 events.log（移动端排查用）。 */
export async function logVpnRoutes(routes: string[]) {
  await invoke("log_vpn_routes", { routes });
}
