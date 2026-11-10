<script setup lang="ts">
import { type } from "@tauri-apps/plugin-os";

import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { open } from "@tauri-apps/plugin-shell";
import { exit } from "@tauri-apps/plugin-process";
import { RemoteManagement, Utils } from "@exvlan/std";
import { useTray } from "~/composables/tray";
import {
  consumePendingMobileVpnTileAction,
  initMobileVpnService,
  mobileVpnPermissionDenied,
  mobileVpnRunning,
  mobileVpnSyncing,
  scheduleVpnAutoSync,
  setMobileVpnTileActionHandler,
  syncMobileVpnService,
} from "~/composables/mobile_vpn";
import { executeVpnTileAction } from "~/composables/mobile_vpn_tile";
import { dedupeNetworkInstances } from "~/composables/instance_dedupe";
import { GUIRemoteClient } from "~/modules/api";
import { logDiagStep, uploadDiagnostics } from "~/composables/backend";

import { useToast, useConfirm } from "openvue";
import { loadMode, saveModeNow, WebClientConfig, type Mode } from "~/composables/mode";
import {
  saveLastNetworkInstanceId,
  saveLastNetworkInstanceIdNow,
  loadLastNetworkInstanceId,
} from "~/composables/config";
import ModeSwitcher from "~/components/ModeSwitcher.vue";
import SettingsDialog from "~/components/SettingsDialog.vue";
import {
  appUpdateApply,
  appUpdateInfo,
  diagnosticsInfo,
  ensureKernelService,
  getServiceStatus,
  readErrorLog,
  reportBootState,
  reportFrontendError,
  serviceDefaultDirs,
  serviceReinstall,
  shareErrorLog,
} from "~/composables/backend";

const { t } = useI18n();
const confirm = useConfirm();
/** 本次启动是否已经自动上传过诊断（进程内状态，重启即重置）。 */
let autoUploadedThisLaunch = false;
/**
 * 安装失败时自动上传诊断（helper / 服务 / 内核安装失败）。
 *
 * 复用「上传诊断」的首次同意开关：没同意过就弹一次应用内确认框，
 * 同意后以后静默上传；不同意就只保留顶部横幅里的错误。
 */
async function maybeAutoUploadDiag(detail: string) {
  if (type() === "ios") {
    return;
  }
  // 每次启动最多自动上传一次：重试/多次失败不再重复打 Worker。
  if (autoUploadedThisLaunch) {
    void logDiagStep("auto_upload_skipped", "already uploaded this launch");
    return;
  }
  autoUploadedThisLaunch = true;
  try {
    if (localStorage.getItem("diag_upload_consent") !== "1") {
      const accepted = await new Promise<boolean>((resolve) => {
        confirm.require({
          header: t("settings.upload_diag"),
          message: t("settings.upload_consent"),
          accept: () => resolve(true),
          reject: () => resolve(false),
        });
      });
      if (!accepted) {
        return;
      }
      localStorage.setItem("diag_upload_consent", "1");
    }
    void logDiagStep("auto_upload_start", detail.slice(0, 200));
    const urls = await uploadDiagnostics("", "install-failed");
    if (urls) {
      toast.add({
        severity: "success",
        summary: t("settings.upload_diag"),
        detail: urls,
        life: 15000,
      });
    }
  } catch (e) {
    void logDiagStep("auto_upload_failed", String(e).slice(0, 300));
    console.warn("auto upload diagnostics failed", e);
  }
}

const aboutVisible = ref(false);
/** 启动/连接失败时保留的错误（顶部横幅展示，不再静默降级模式）。 */
const startupError = ref("");

/** 补齐 service 模式缺失的默认值（历史版本可能只存了 mode 字段）。 */
async function normalizeMode(mode: Mode): Promise<Mode> {
  if (mode.mode !== "service") {
    return mode;
  }
  const svc = mode as unknown as Record<string, any>;
  const saved = loadMode() as unknown as Record<string, any>;
  const defaults = await serviceDefaultDirs();
  const configDir = svc.config_dir || saved.config_dir || defaults.config_dir;
  const logDir = svc.file_log_dir || saved.file_log_dir || defaults.file_log_dir;
  return {
    ...mode,
    config_dir: configDir,
    file_log_dir: logDir,
    file_log_level: svc.file_log_level || saved.file_log_level || "info",
    rpc_portal: svc.rpc_portal || saved.rpc_portal || "127.0.0.1:15999",
  } as Mode;
}

async function onCopyDiagnostics() {
  try {
    const text = await readErrorLog(512);
    await writeText(text || "");
    toast.add({
      severity: "success",
      summary: t("web.common.success"),
      detail: t("settings.copy"),
      life: 2000,
    });
  } catch (e: any) {
    toast.add({ severity: "error", summary: t("error"), detail: String(e), life: 6000 });
  }
}

async function onShareErrorLog() {
  try {
    const path = await shareErrorLog();
    if (type() === "android") {
      // 分享面板不可用时，Rust 会兜底落盘到「下载」并以 `saved: ` 前缀返回。
      if (path.startsWith("saved: ")) {
        toast.add({
          severity: "warn",
          summary: t("client.share_error_log"),
          detail: `分享不可用，已保存到 ${path.slice(7)}`,
          life: 15000,
        });
      }
      return;
    }
    await writeText(path);
    toast.add({
      severity: "info",
      summary: t("client.share_error_log"),
      detail: path,
      life: 6000,
    });
    return;
  } catch (e: any) {
    // 系统分享不可用（插件命令缺失/无分享目标）：兜底把 error.log 复制到剪贴板
    console.warn("share diagnostics failed, fallback to clipboard", e);
    try {
      const text = await readErrorLog(512);
      if (text) {
        await writeText(text);
        toast.add({
          severity: "warn",
          summary: t("client.share_error_log"),
          detail: `分享不可用（${String(e)}）；已把 error.log 复制到剪贴板，直接粘贴即可`,
          life: 12000,
        });
        return;
      }
    } catch (inner) {
      console.error("clipboard fallback failed", inner);
    }
    toast.add({ severity: "error", summary: t("error"), detail: String(e), life: 10000 });
  }
}

const reinstalling = ref(false);
const settingsSection = ref<"general" | "network" | "logging" | "about">("general");

function openServiceCheck() {
  settingsSection.value = "network";
  settingsVisible.value = true;
}

async function onReinstallService() {
  reinstalling.value = true;
  try {
    await serviceReinstall({
      config_dir: "",
      rpc_portal: (currentMode.value as any).rpc_portal || "127.0.0.1:15999",
      file_log_level: (currentMode.value as any).file_log_level || "info",
      file_log_dir: "",
      config_server: (currentMode.value as any).config_server_url,
    });
    toast.add({
      severity: "success",
      summary: t("web.common.success"),
      detail: t("settings.service_reinstall"),
      life: 3000,
    });
    await retryStartup();
  } catch (e: any) {
    toast.add({ severity: "error", summary: t("error"), detail: String(e), life: 10000 });
  } finally {
    reinstalling.value = false;
  }
}

async function retryStartup() {
  startupError.value = "";
  try {
    await ensureLocalKernel(currentMode.value);
  } catch (e: any) {
    startupError.value = String(e);
    toast.add({ severity: "error", summary: t("error"), detail: String(e), life: 10000 });
    return;
  }
  await initWithMode(currentMode.value);
}
const modeDialogVisible = ref(false);
const appUpdateDialogVisible = ref(false);
const appUpdateVersion = ref("");
const appUpdateNotes = ref("");
const appUpdating = ref(false);

const currentMode = ref<Mode>({ mode: "normal" });
const editingMode = ref<Mode>({ mode: "normal" });
const isModeSaving = ref(false);
const manualDisconnect = ref(false);

const settingsVisible = ref(false);
const logLevel = ref(localStorage.getItem("log_level") || "info");
const runningInstances = ref<string[]>([]);
const vpnRunning = computed(() => runningInstances.value.length > 0);

/** 移动端：实例在跑但 VpnService 没起来（内核通了、流量却没进隧道）。 */
const isMobileDevice = computed(() => type() === "android" || type() === "ios");
const vpnBannerDismissed = ref(false);
// 实例起来但 VpnService 没接管时自动补启动（1s/3s/9s 退避，最多 3 次）。
watch(runningInstances, (list) => {
  if (!isMobileDevice.value || list.length === 0) {
    return;
  }
  if (manualDisconnect.value || mobileVpnRunning.value) {
    return;
  }
  scheduleVpnAutoSync("instances-changed");
});

const showVpnNotRunningBanner = computed(
  () =>
    isMobileDevice.value &&
    runningInstances.value.length > 0 &&
    !mobileVpnRunning.value &&
    !vpnBannerDismissed.value,
);

async function onStartVpnFromBanner() {
  try {
    await syncMobileVpnService();
    if (!mobileVpnRunning.value) {
      toast.add({
        severity: "warn",
        summary: t("settings.vpn_not_connected"),
        detail: t("settings.vpn_not_connected_hint"),
        life: 8000,
      });
    }
  } catch (e: any) {
    toast.add({ severity: "error", summary: t("error"), detail: String(e), life: 10000 });
  }
}

async function refreshTrayMenu() {
  if (type() === "android" || type() === "ios") {
    return;
  }
  await setTrayMenu([await MenuItemShow(t("tray.show")), await MenuItemExit(t("tray.exit"))]);
}

async function getLogDirPath(): Promise<string> {
  return await invoke<string>("get_log_dir_path");
}

async function onOpenLogDir() {
  await open(await getLogDirPath());
}

async function onCopyLogDir() {
  await writeText(await getLogDirPath());
}

async function onSetLogLevel(level: string) {
  logLevel.value = level;
  localStorage.setItem("log_level", level);
  await setLoggingLevel(level);
}

const configServerDialogVisible = ref(false);
const configServerConnected = ref(false);

const showAutostartHint = ref(false);

async function openModeDialog() {
  editingMode.value = JSON.parse(JSON.stringify(loadMode()));
  showAutostartHint.value = false;
  modeDialogVisible.value = true;
}

async function openAutostartDialog() {
  editingMode.value = JSON.parse(JSON.stringify(loadMode()));
  editingMode.value.mode = "service";
  showAutostartHint.value = true;
  modeDialogVisible.value = true;
}

async function onModeSave() {
  if (isModeSaving.value) {
    return;
  }
  // 首次切到普通模式：确认一次（会通过特权组件以管理员身份启动内核）。
  if (editingMode.value.mode === "normal" && localStorage.getItem("normal_mode_consent") !== "1") {
    const accepted = await new Promise<boolean>((resolve) => {
      confirm.require({
        header: t("mode.normal"),
        message: t("mode.normal_consent"),
        icon: "pi pi-exclamation-triangle",
        accept: () => resolve(true),
        reject: () => resolve(false),
      });
    });
    if (!accepted) {
      return;
    }
    localStorage.setItem("normal_mode_consent", "1");
  }
  isModeSaving.value = true;
  try {
    // 先落盘并等待后端确认，再做连接（连接失败也保留用户选择）。
    await saveModeNow(editingMode.value);
    // 切模式后内核的配置目录/实例集合可能不同：先清空上次选择，
    // 由 initWithMode 按新内核的真实列表恢复，避免对着不存在的实例反复 get_config。
    instanceId.value = undefined;
    await initWithMode(editingMode.value);
    modeDialogVisible.value = false;
  } catch (e: any) {
    // 不再用旧模式重新初始化：那会把刚保存的模式写回旧值（历史 bug）。
    // 保留用户选择，只在横幅/日志里说明原因。
    startupError.value = String(e);
    void reportFrontendError("fe:mode-save", String(e));
    toast.add({
      severity: "error",
      summary: t("client.startup_failed"),
      detail: String(e),
      life: 10000,
    });
    console.error("Error switching mode", e, currentMode.value, editingMode.value);
  } finally {
    isModeSaving.value = false;
  }
}

async function onUninstallService() {
  confirm.require({
    message: t("mode.uninstall_service_confirm"),
    header: t("mode.uninstall_service"),
    icon: "pi pi-exclamation-triangle",
    rejectProps: {
      label: t("web.common.cancel"),
      severity: "secondary",
      outlined: true,
    },
    acceptProps: {
      label: t("mode.uninstall_service"),
      severity: "danger",
    },
    accept: async () => {
      isModeSaving.value = true;
      try {
        await initWithMode({ ...currentMode.value, mode: "normal" });
        await initService(undefined);
        toast.add({
          severity: "success",
          summary: t("web.common.success"),
          detail: t("mode.uninstall_service_success"),
          life: 3000,
        });
        modeDialogVisible.value = false;
      } catch (e: any) {
        toast.add({ severity: "error", summary: t("error"), detail: e, life: 10000 });
        console.error("Error uninstalling service", e);
      } finally {
        isModeSaving.value = false;
      }
    },
  });
}

async function onStopService() {
  isModeSaving.value = true;
  manualDisconnect.value = true;
  try {
    await setServiceStatus(false);
    toast.add({
      severity: "success",
      summary: t("web.common.success"),
      detail: t("mode.stop_service_success"),
      life: 3000,
    });
    modeDialogVisible.value = false;
  } catch (e: any) {
    toast.add({ severity: "error", summary: t("error"), detail: e, life: 10000 });
    console.error("Error stopping service", e);
  } finally {
    isModeSaving.value = false;
  }
}

async function initWithMode(rawMode: Mode) {
  const mode = await normalizeMode(rawMode);
  const running_inst_ids =
    (await remoteClient.value.list_network_instance_ids().catch(() => undefined))
      ?.running_inst_ids ?? [];

  // 新架构：service 与 normal 共用同一个常驻 root 守护进程，
  // 区别仅在于「退出应用时是否停实例」。因此切模式**不再**停/卸载服务
  // （旧逻辑会因状态不匹配而失败，且会破坏 normal 模式所需的 TUN）。

  let url: string | undefined = undefined;
  let retrys = 1;
  switch (mode.mode) {
    case "remote":
      if (!mode.remote_rpc_address) {
        // 不再静默降级为普通模式：保留用户选择，失败原因留在顶部横幅。
        startupError.value = t("mode.remote_rpc_address_empty");
        toast.add({
          severity: "error",
          summary: t("client.startup_failed"),
          detail: startupError.value,
          life: 10000,
        });
        return;
      }
      url = mode.remote_rpc_address;
      break;
    case "service": {
      if (!mode.config_dir || !mode.file_log_dir || !mode.file_log_level || !mode.rpc_portal) {
        startupError.value = t("mode.service_config_empty");
        toast.add({
          severity: "error",
          summary: t("client.startup_failed"),
          detail: startupError.value,
          life: 10000,
        });
        return;
      }
      // 是否（重）装服务只由**服务指纹**决定：内核路径 + 启动参数完全一致时
      // ensureKernelService 直接返回 current，不会触发授权（历史 bug：用版本号比较，
      // 而 exvlan_version 现在返回内核版本，导致每次启动都判定需要重装 → 反复弹密码）。
      mode.config_server_url = mode.config_server_url || undefined;
      // 与启动时同一个入口：内部会补默认值、按指纹判断是否真的需要提权。
      await ensureLocalKernel(mode);
      let serviceStatus = await getServiceStatus();
      if (serviceStatus === "Stopped") {
        await setServiceStatus(true);
      }
      url = "tcp://" + mode.rpc_portal.replace("0.0.0.0", "127.0.0.1");
      retrys = 5;
      break;
    }
    case "normal":
      url = mode.rpc_portal;
      break;
  }
  for (let i = 0; i < retrys; i++) {
    try {
      await connectRpcClient(mode.mode === "normal", url);
      break;
    } catch (e) {
      if (i === retrys - 1) {
        const errMsg = e instanceof Error ? e.message : String(e);
        startupError.value = t("mode.rpc_connection_failed", { error: errMsg });
        void reportFrontendError(
          "fe:rpc-connect",
          errMsg,
          e instanceof Error ? e.stack : undefined,
        );
        toast.add({
          severity: "error",
          summary: t("client.startup_failed"),
          detail: startupError.value,
          life: 10000,
        });
        throw e;
      }
      console.error("Error connecting rpc client, retrying...", e);
      await new Promise((resolve) => setTimeout(resolve, 1000));
    }
  }
  await sendConfigs(running_inst_ids.map(Utils.UuidToStr));
  if (mode.mode === "normal") {
    mode.config_server_url = mode.config_server_url || undefined;
    initWebClient(mode.config_server_url);
  }
  currentMode.value = mode;
  await saveModeNow(mode);
  startupError.value = "";
  clientRunning.value = await isClientRunning();

  // 连接成功后再恢复一次上次网络（不依赖 clientRunning 的 false→true 翻转）
  // 注意：切模式后配置目录不同，上次的网络 ID 可能在新内核里不存在，
  // 直接使用会导致前端反复 `get_config` 失败（日志里的 `Not found: No such network instance`）。
  const lastId = await knownInstanceOrDrop(loadLastNetworkInstanceId());
  instanceId.value = lastId;
}

/** 校验「上次选择的网络」在当前内核里是否真的存在。 */
async function knownInstanceOrDrop(
  candidate: string | null | undefined,
): Promise<string | undefined> {
  if (!candidate) {
    return undefined;
  }
  try {
    const ids = await remoteClient.value.list_network_instance_ids();
    const known = [...(ids.running_inst_ids ?? []), ...(ids.disabled_inst_ids ?? [])].map(
      Utils.UuidToStr,
    );
    if (known.length === 0 || known.includes(candidate)) {
      return candidate;
    }
    void logDiagStep("instance_stale", `drop ${candidate} (known=${known.length})`);
    return undefined;
  } catch {
    return candidate;
  }
}

/** 桌面端启动闭环：确保内核已安装、服务定义与当前配置一致（一致时不提权）。 */
/**
 * 自动提权闸门。
 *
 * - 用户取消过 UAC → 不再自动弹（只弹一次），改为横幅提示 + 设置里手动重试
 * - 失败每会话最多自动尝试 3 次，避免反复弹窗
 */
const autoElevateAttempts = ref(0);
const elevateDenied = ref(false);

async function loadElevateDenied() {
  try {
    const state = await invoke<Record<string, string>>("get_ui_state");
    elevateDenied.value = state?.elevate_denied === "1";
  } catch {
    // 读不到就当作未拒绝
  }
}

function isElevateCancel(err: unknown): boolean {
  const text = String(err).toLowerCase();
  return text.includes("cancel") || text.includes("取消");
}

async function ensureKernelServiceGuarded(opts: {
  config_dir: string;
  rpc_portal: string;
  file_log_level: string;
  file_log_dir: string;
  config_server?: string;
}) {
  if (elevateDenied.value) {
    startupError.value ||= t("settings.elevate_denied_hint");
    return;
  }
  if (autoElevateAttempts.value >= 3) {
    startupError.value ||= t("settings.elevate_retry_exhausted");
    return;
  }
  autoElevateAttempts.value += 1;
  try {
    await ensureKernelService(opts);
  } catch (e: any) {
    if (isElevateCancel(e)) {
      elevateDenied.value = true;
      void logDiagStep("elevate_denied", String(e).slice(0, 200));
      void invoke("set_ui_state", { key: "elevate_denied", value: "1" }).catch(() => {});
      startupError.value = t("settings.elevate_denied_hint");
    }
    throw e;
  }
}

async function ensureLocalKernel(mode: Mode) {
  const platform = type();
  if (platform === "android" || platform === "ios") return;
  if (mode.mode === "remote") return;

  // daemon 参数与模式无关：优先取当前 mode 上的字段（service 模式有），
  // 否则回退到已保存的 app_mode 里的同名字段，最后才用默认值。
  const saved = loadMode() as {
    config_dir?: string;
    rpc_portal?: string;
    file_log_level?: string;
    file_log_dir?: string;
  };
  // 默认目录必须是系统路径：守护进程以 root 运行，写用户目录会把属主改成 root，
  // 之后 GUI 连 error.log / ui-state.json 都写不进去（20709 实测）。
  // 普通模式只装内核（不装系统服务）：这是唯一一次提权，之后内核以用户身份跑。
  if (mode.mode === "normal") {
    try {
      await invoke<string>("ensure_kernel_only");
    } catch (e: any) {
      console.error("ensure_kernel_only failed", e);
    }
    return;
  }

  const defaults = await serviceDefaultDirs();
  const merged = {
    config_dir: (mode as any).config_dir || saved.config_dir || defaults.config_dir,
    rpc_portal: (mode as any).rpc_portal || saved.rpc_portal || "127.0.0.1:15999",
    file_log_level: (mode as any).file_log_level || saved.file_log_level || "info",
    file_log_dir: (mode as any).file_log_dir || saved.file_log_dir || defaults.file_log_dir,
    config_server: (mode as { config_server_url?: string }).config_server_url,
  };
  await ensureKernelServiceGuarded(merged);
}

/** 应用更新：启动静默检查（24h 节流，已忽略的版本不再打扰）。 */
async function maybeCheckAppUpdate() {
  const KEY_LAST = "upgrade_last_check";
  const KEY_IGNORED = "upgrade_ignored_version";

  const last = Number(localStorage.getItem(KEY_LAST) || 0);
  if (Date.now() - last < 24 * 3600 * 1000) {
    return;
  }

  try {
    const info = await appUpdateInfo();
    localStorage.setItem(KEY_LAST, String(Date.now()));
    if (!info.available || !info.latest) {
      return;
    }
    if (localStorage.getItem(KEY_IGNORED) === info.latest) {
      return;
    }
    appUpdateVersion.value = info.latest;
    appUpdateNotes.value = info.notes || "";
    appUpdateDialogVisible.value = true;
  } catch (e) {
    // 更新源未同步/离线：静默
    console.warn("app update check failed", e);
  }
}

async function onAppUpdateNow() {
  if (!appUpdateVersion.value) {
    return;
  }
  appUpdating.value = true;
  try {
    await appUpdateApply(appUpdateVersion.value);
    appUpdateDialogVisible.value = false;
  } catch (e: any) {
    toast.add({ severity: "error", summary: t("error"), detail: String(e), life: 10000 });
  } finally {
    appUpdating.value = false;
  }
}

function onAppUpdateLater() {
  localStorage.setItem("upgrade_ignored_version", appUpdateVersion.value);
  appUpdateDialogVisible.value = false;
}

onMounted(async () => {
  const cleanupFns: Array<() => void> = [];

  // 提权被拒绝过就不再自动弹 UAC（见 ensureKernelServiceGuarded）
  await loadElevateDenied();

  // `logLevel` 的初始值在模块加载时读取，此时 main.ts 的 prefs 回填还没执行；
  // 这里再读一次，保证 UI 与 prefs.toml 一致。
  logLevel.value = localStorage.getItem("log_level") || "info";

  if (type() === "android") {
    try {
      await initMobileVpnService();
    } catch (e: any) {
      console.error("exvlan init vpn service failed", e);
    }
  }

  cleanupFns.push(await listenGlobalEvents());
  currentMode.value = await normalizeMode(loadMode());

  // 模式丢失（prefs 与 localStorage 都没有）且内核服务已在运行 → 让用户选一次。
  // 历史 bug：prefs 的 app_mode 被并发写覆盖后，每次启动都静默回到普通模式。
  let modeSource = localStorage.getItem("app_mode") === null ? "default" : "persisted";
  if (modeSource === "default" && type() !== "android" && type() !== "ios") {
    const status = await getServiceStatus().catch(() => "NotInstalled");
    if (status === "Running") {
      const useService = await new Promise<boolean>((resolve) => {
        confirm.require({
          header: t("settings.mode"),
          message: t("settings.mode_missing_service_running"),
          accept: () => resolve(true),
          reject: () => resolve(false),
        });
      });
      const chosen = await normalizeMode(
        (useService ? { mode: "service" } : { mode: "normal" }) as Mode,
      );
      await saveModeNow(chosen);
      currentMode.value = chosen;
      modeSource = "dialog";
    }
  }
  void reportBootState(currentMode.value.mode, loadLastNetworkInstanceId(), null, modeSource);

  // 首次安装/升级后会自动装好内核并（重新）注册系统服务；失败不阻塞进入界面。
  // service 模式由 initWithMode 内部统一处理，避免同一次启动弹两次授权。
  if (currentMode.value.mode !== "service") {
    try {
      await ensureLocalKernel(currentMode.value);
    } catch (e: any) {
      console.error("ensure kernel/service failed", e);
      startupError.value = String(e);
      void maybeAutoUploadDiag(String(e));
      void reportFrontendError("fe:ensure-kernel", String(e));
      toast.add({
        severity: "error",
        summary: t("client.startup_failed"),
        detail: String(e),
        life: 10000,
      });
    }
  }

  // 目录被 root 占用时（历史版本守护进程写过用户目录），日志与设置都写不进去。
  try {
    const diag = await diagnosticsInfo();
    if (diag.log_dir_writable === false || diag.config_dir_writable === false) {
      startupError.value = `日志/配置目录不可写（可能被 root 占用）：${diag.log_dir ?? "-"} / ${diag.config_dir ?? "-"}`;
    }
  } catch (e) {
    console.error("diagnostics failed", e);
  }

  try {
    await initWithMode(currentMode.value);
    // 服务启动参数固定 info；用户的日志级别在连接成功后再热更新（不进指纹）。
    void setLoggingLevel(logLevel.value).catch((e) => {
      console.warn("apply log level failed", e);
    });
    void dedupeNetworkInstances().catch((e) => {
      console.warn("dedupe network instances failed", e);
    });
  } catch (e: any) {
    // 保留用户所选模式，只把原因留在顶部横幅与 error.log
    console.error("initWithMode failed", e);
    startupError.value ||= String(e);
    void maybeAutoUploadDiag(String(e));
  }
  void maybeCheckAppUpdate();

  if (type() === "android") {
    setMobileVpnTileActionHandler(handleMobileVpnTileAction);
    cleanupFns.push(() => setMobileVpnTileActionHandler());
    try {
      await consumePendingMobileVpnTileAction();
      await syncMobileVpnService();
    } catch (e: any) {
      console.error("exvlan sync vpn service failed", e);
    }
  }

  onUnmounted(() => {
    cleanupFns.forEach((unlisten) => unlisten());
  });
});

useTray(type() !== "android" && type() !== "ios");
let toast = useToast();

const remoteClient = computed(() => new GUIRemoteClient());
const instanceId = ref<string | undefined>(undefined);
const clientRunning = ref(false);

async function handleMobileVpnTileAction(action: "start" | "stop") {
  try {
    const result = await executeVpnTileAction(action, remoteClient.value, {
      lastInstanceId: loadLastNetworkInstanceId(),
      syncVpnService: syncMobileVpnService,
    });

    if (!result.instanceId) {
      toast.add({
        severity: "warn",
        summary: t("vpn_tile_no_network"),
        detail: t("vpn_tile_no_network_description"),
        life: 5000,
      });
      return;
    }

    instanceId.value = result.instanceId;
    saveLastNetworkInstanceId(result.instanceId);
    toast.add({
      severity: action === "start" ? "success" : "secondary",
      summary: t(action === "start" ? "vpn_tile_started" : "vpn_tile_stopped"),
      life: 3000,
    });
  } catch (error) {
    console.error("VPN tile action failed", action, error);
    toast.add({
      severity: "error",
      summary: t("error"),
      detail: t("vpn_tile_action_failed", { error: String(error) }),
      life: 8000,
    });
  }
}

watch(instanceId, (newVal) => {
  if (newVal) {
    saveLastNetworkInstanceId(newVal);
    void saveLastNetworkInstanceIdNow(newVal);
  }
});

watch(clientRunning, async (newVal, oldVal) => {
  if (!newVal && oldVal) {
    if (manualDisconnect.value) {
      manualDisconnect.value = false;
      return;
    }
    await reconnectClient();
  } else if (newVal && !oldVal) {
    const lastInstanceId = loadLastNetworkInstanceId();
    if (lastInstanceId) {
      instanceId.value = lastInstanceId;
    }
    void dedupeNetworkInstances().catch((e) => {
      console.warn("dedupe network instances failed", e);
    });
  }
});

onMounted(async () => {
  clientRunning.value = await isClientRunning().catch(() => false);
  // 直接按上次选择恢复网络，避免「连不上 → 首页显示未选中网络 → 需要手动选」
  const lastInstanceId = loadLastNetworkInstanceId();
  if (lastInstanceId) {
    instanceId.value = lastInstanceId;
  }
  const timer = setInterval(async () => {
    try {
      clientRunning.value = await isClientRunning();
    } catch (e) {
      clientRunning.value = false;
      console.error("Error checking client running status", e);
    }
  }, 1000);

  onUnmounted(() => {
    clearInterval(timer);
  });
});
async function reconnectClient() {
  editingMode.value = JSON.parse(JSON.stringify(loadMode()));
  await onModeSave();
}

onMounted(async () => {
  window.setTimeout(async () => {
    await refreshTrayMenu();
  }, 1000);

  // 底栏「运行」灯：只看是否有实例在跑（5s 一次，未连接时直接清零）
  const timer = setInterval(async () => {
    if (!clientRunning.value) {
      runningInstances.value = [];
      return;
    }
    try {
      const resp = await remoteClient.value.list_network_instance_ids();
      runningInstances.value = (resp.running_inst_ids ?? []).map(Utils.UuidToStr);
    } catch {
      runningInstances.value = [];
    }
  }, 5000);
  onUnmounted(() => clearInterval(timer));
});

async function connectRpcClient(isNormalMode: boolean, url?: string) {
  await initRpcConnection(isNormalMode, url);
  console.log("exvlan rpc connection established, isNormalMode: ", isNormalMode);
}

async function openConfigServerDialog() {
  editingMode.value = JSON.parse(JSON.stringify(loadMode()));
  configServerDialogVisible.value = true;
}
async function onConfigServerSave() {
  if (JSON.stringify(currentMode.value) === JSON.stringify(editingMode.value)) {
    configServerDialogVisible.value = false;
    return;
  }
  if (editingMode.value.mode === "service") {
    await new Promise<void>((resolve, reject) => {
      confirm.require({
        message: t("config-server.update_service_confirm"),
        icon: "pi pi-exclamation-triangle",
        rejectProps: {
          label: t("web.common.cancel"),
          severity: "secondary",
          outlined: true,
        },
        acceptProps: {
          label: t("web.common.confirm"),
        },
        accept: async () => {
          resolve();
        },
        reject: () => {
          reject();
        },
      });
    });
  }
  console.log("Saving config server url", (editingMode.value as WebClientConfig).config_server_url);
  await onModeSave();
  configServerDialogVisible.value = false;
}
onMounted(() => {
  const timer = setInterval(async () => {
    if (currentMode.value.mode !== "normal") return;
    if (!currentMode.value.config_server_url) return;
    configServerConnected.value = await isWebClientConnected();
  }, 1000);

  onUnmounted(() => {
    clearInterval(timer);
  });
});
const configServerConnectionStatus = computed(() => {
  if (currentMode.value.mode !== "normal") {
    return "unknown";
  }
  if (!currentMode.value.config_server_url) {
    return "disconnected";
  }
  return configServerConnected.value ? "connected" : "connecting";
});
</script>

<template>
  <div id="root" class="flex flex-col">
    <Dialog
      v-model:visible="aboutVisible"
      modal
      :header="t('about.title')"
      :style="{ width: '70%' }"
    >
      <About />
    </Dialog>

    <Dialog
      v-model:visible="appUpdateDialogVisible"
      modal
      :header="t('upgrade.app_version')"
      :style="{ width: '40vw' }"
    >
      <div class="flex flex-col gap-2">
        <p>{{ t("upgrade.new_version", { version: appUpdateVersion }) }}</p>
        <p v-if="appUpdateNotes" class="text-sm whitespace-pre-wrap text-gray-500">
          {{ appUpdateNotes }}
        </p>
      </div>
      <template #footer>
        <Button :label="t('web.common.cancel')" text @click="onAppUpdateLater" />
        <Button
          :label="t('upgrade.download_and_install')"
          :loading="appUpdating"
          @click="onAppUpdateNow"
        />
      </template>
    </Dialog>
    <Dialog
      v-model:visible="modeDialogVisible"
      modal
      :header="t('mode.switch_mode')"
      :style="{ width: '50vw' }"
    >
      <Message v-if="showAutostartHint" severity="info" :closable="false" class="mb-4">
        {{ t("mode.autostart_hint") }}
      </Message>
      <ModeSwitcher
        v-model="editingMode"
        @uninstall-service="onUninstallService"
        @stop-service="onStopService"
      />
      <template #footer>
        <Button
          :label="t('web.common.cancel')"
          icon="pi pi-times"
          @click="modeDialogVisible = false"
          text
        />
        <Button
          :label="t('web.common.save')"
          icon="pi pi-save"
          @click="onModeSave"
          autofocus
          :loading="isModeSaving"
        />
      </template>
    </Dialog>

    <Dialog
      v-model:visible="configServerDialogVisible"
      modal
      :header="t('config-server.title')"
      :style="{ width: '50vw' }"
    >
      <div class="flex flex-col gap-3">
        <label for="config-server-address">{{ t("config-server.address") }}</label>
        <InputText
          id="config-server-address"
          v-model="(editingMode as WebClientConfig).config_server_url"
          :placeholder="t('config-server.address_placeholder')"
        />
        <small class="p-text-secondary whitespace-pre-wrap">{{
          t("config-server.description")
        }}</small>
      </div>
      <template #footer>
        <Button
          :label="t('web.common.cancel')"
          icon="pi pi-times"
          @click="configServerDialogVisible = false"
          text
        />
        <Button
          :label="t('web.common.save')"
          icon="pi pi-save"
          @click="onConfigServerSave"
          autofocus
          :loading="isModeSaving"
        />
      </template>
    </Dialog>

    <div
      v-if="startupError"
      class="flex items-center gap-2 border-l-4 border-red-500 bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
    >
      <i class="pi pi-exclamation-triangle" />
      <span class="flex-1 break-all">{{ t("client.startup_failed") }}: {{ startupError }}</span>
      <Button
        size="small"
        :label="t('client.retry_startup')"
        icon="pi pi-replay"
        @click="retryStartup"
      />
      <Button
        size="small"
        text
        :label="t('settings.service_check')"
        icon="pi pi-wrench"
        @click="openServiceCheck"
      />
      <Button
        size="small"
        text
        :label="t('settings.service_reinstall')"
        icon="pi pi-refresh"
        :loading="reinstalling"
        @click="onReinstallService"
      />
      <Button
        size="small"
        text
        :label="t('settings.copy')"
        icon="pi pi-copy"
        @click="onCopyDiagnostics"
      />
      <Button
        size="small"
        text
        :label="t('client.share_error_log')"
        icon="pi pi-share-alt"
        @click="onShareErrorLog"
      />
    </div>

    <div
      v-if="showVpnNotRunningBanner"
      class="flex items-center gap-2 border-l-4 border-amber-500 bg-amber-50 px-3 py-2 text-sm text-amber-900 dark:bg-amber-950 dark:text-amber-200"
    >
      <i class="pi pi-exclamation-circle" />
      <span class="flex-1">{{
        mobileVpnPermissionDenied
          ? t("settings.vpn_need_permission")
          : t("settings.vpn_not_connected_hint")
      }}</span>
      <Button
        size="small"
        :label="t('settings.vpn_start_now')"
        icon="pi pi-play"
        :loading="mobileVpnSyncing"
        @click="onStartVpnFromBanner"
      />
      <Button
        size="small"
        text
        :label="t('web.common.cancel')"
        @click="vpnBannerDismissed = true"
      />
    </div>

    <RemoteManagement
      v-if="clientRunning"
      class="flex-1 overflow-y-auto"
      :api="remoteClient"
      :pause-auto-refresh="isModeSaving"
      v-model:instance-id="instanceId"
    />
    <div v-else class="empty-state flex-1 flex flex-col items-center py-12">
      <i class="pi pi-server text-5xl text-secondary mb-4 opacity-50"></i>
      <div class="text-xl text-center font-medium mb-3">{{ t("client.not_running") }}</div>
      <Button
        @click="reconnectClient"
        :loading="isModeSaving"
        :label="t('client.retry')"
        icon="pi pi-replay"
        iconPos="left"
      />
    </div>

    <div
      class="flex items-center gap-3 px-3 py-2 border-t border-surface-200 dark:border-surface-700"
    >
      <span class="flex items-center gap-1 text-sm">
        <i
          class="pi pi-circle-fill text-[10px]"
          :class="clientRunning ? 'text-green-500' : 'text-red-500'"
        />
        <span class="text-surface-500">
          {{ clientRunning ? t("settings.rpc_connected") : t("settings.rpc_disconnected") }}
        </span>
      </span>
      <span class="flex items-center gap-1 text-sm">
        <i
          class="pi pi-circle-fill text-[10px]"
          :class="vpnRunning ? 'text-green-500' : 'text-surface-400'"
        />
        <span class="text-surface-500">
          {{ vpnRunning ? t("settings.vpn_running") : t("settings.vpn_stopped") }}
        </span>
      </span>
      <span class="text-sm text-surface-500">{{ t("mode." + currentMode.mode) }}</span>
      <span class="flex-1" />
      <Button
        icon="pi pi-cog"
        text
        rounded
        :aria-label="t('settings.title')"
        @click="settingsVisible = true"
      />
      <Button
        icon="pi pi-power-off"
        text
        rounded
        severity="danger"
        :aria-label="t('exit')"
        @click="exit(1)"
      />
    </div>

    <SettingsDialog
      v-model:visible="settingsVisible"
      :initial-section="settingsSection"
      :mode="currentMode"
      :client-running="clientRunning"
      :config-server-status="configServerConnectionStatus"
      :log-level="logLevel"
      @open-mode="openModeDialog"
      @open-autostart="openAutostartDialog"
      @open-config-server="openConfigServerDialog"
      @open-log-dir="onOpenLogDir"
      @copy-log-dir="onCopyLogDir"
      @share-log="onShareErrorLog"
      @set-log-level="onSetLogLevel"
      @changed-language="refreshTrayMenu"
    />
  </div>
</template>

<style scoped lang="postcss">
#root {
  height: 100vh;
  width: 100vw;
}

.p-dropdown :deep(.p-dropdown-panel .p-dropdown-items .p-dropdown-item) {
  padding: 0 0.5rem;
}
</style>

<style>
body {
  height: 100vh;
  width: 100vw;
  padding: 0;
  margin: 0;
  overflow: hidden;
}

.p-menubar .p-menuitem {
  margin: 0;
}

.p-select-overlay {
  max-width: calc(100% - 2rem);
}

/*

.p-tabview-panel {
  height: 100%;
} */
</style>
