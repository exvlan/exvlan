<script setup lang="ts">
import { type } from "@tauri-apps/plugin-os";

import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { open } from "@tauri-apps/plugin-shell";
import { exit } from "@tauri-apps/plugin-process";
import { RemoteManagement, Utils } from "@exvlan/std";
import { MenuItem, PredefinedMenuItem, Submenu } from "@tauri-apps/api/menu";
import { MenuItemExit, MenuItemShow, useTray, setTrayMenu } from "~/composables/tray";
import {
  consumePendingMobileVpnTileAction,
  initMobileVpnService,
  mobileVpnPermissionDenied,
  mobileVpnRouteError,
  mobileVpnRunning,
  mobileVpnSyncing,
  scheduleVpnAutoSync,
  setMobileVpnTileActionHandler,
  syncMobileVpnService,
} from "~/composables/mobile_vpn";
import { executeVpnTileAction } from "~/composables/mobile_vpn_tile";
import { dedupeNetworkInstances } from "~/composables/instance_dedupe";
import { GUIRemoteClient } from "~/modules/api";
import { GatewayRemoteClient } from "~/modules/gateway_api";
import {
  gatewayRemotes,
  gatewaySystem,
  getUpdatePrefs,
  logDiagStep,
  uploadDiagnostics,
} from "~/composables/backend";
import { buildFrontendState } from "~/composables/diagnostics";

import { useToast, useConfirm } from "openvue";
import { loadMode, WebClientConfig, type Mode } from "~/composables/mode";
import {
  DEFAULT_RPC_PORT,
  LOCAL_TARGET,
  type RuntimeKind,
  findRemote,
  loadRemotes,
  loadRuntime,
  loadTargetId,
  remoteAddress,
  remoteMode,
  remoteUrl,
  saveRuntime,
  saveTargetId,
} from "~/composables/target";
import {
  saveLastNetworkInstanceId,
  saveLastNetworkInstanceIdNow,
  loadLastNetworkInstanceId,
} from "~/composables/config";
import TargetSwitcher from "~/components/TargetSwitcher.vue";
import ServiceSettings from "~/components/ServiceSettings.vue";
import SettingsDialog from "~/components/SettingsDialog.vue";
import {
  runtimeApply,
  serviceKernelRestart,
  serviceWhitelistInfo,
  appUpdateApply,
  appUpdateInfo,
  diagnosticsInfo,
  ensureKernelService,
  readErrorLog,
  reportBootState,
  reportFrontendError,
  serviceDefaultDirs,
  serviceReinstall,
  shareErrorLog,
  serviceState,
  serviceInstall,
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
          // ConfirmPopup 需要锚点；启动时的同意框没有点击事件，锚到页面根节点。
          target: (document.querySelector("#root") as HTMLElement | null) ?? document.body,
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

/**
 * helper 就绪状态（macOS 特权组件）。缺失时：顶部横幅提示 + 「重试安装」。
 *
 * 启动后**后台自动重试一次**（会弹一次系统授权）：大多数情况是上次升级中途退出，
 * 自动补装即可恢复；失败后不再自动打扰，交给用户手动点。
 */
const serviceReady = ref(true);
const serviceStatusText = ref("");
const retryingService = ref(false);
let serviceAutoRetried = false;

async function refreshServiceState() {
  if (type() === "android" || type() === "ios") {
    return;
  }
  try {
    const st = await serviceState();
    serviceReady.value = st.ready;
    serviceStatusText.value = st.status || "";
    if (!st.ready && !serviceAutoRetried) {
      serviceAutoRetried = true;
      void logDiagStep("service_auto_retry", st.status.slice(0, 120));
      void onRetryService(true);
    }
  } catch {
    // 读不到状态就当作就绪，避免误报横幅
    serviceReady.value = true;
  }
}

async function onRetryService(auto = false) {
  if (retryingService.value) {
    return;
  }
  retryingService.value = true;
  try {
    await serviceInstall();
    toast.add({
      severity: "success",
      summary: t("mode.privileged_service"),
      detail: t("mode.service_retry_ok"),
      life: 6000,
    });
  } catch (e: any) {
    void logDiagStep("service_retry_failed", String(e).slice(0, 300));
    toast.add({
      severity: auto ? "warn" : "error",
      summary: t("mode.privileged_service"),
      detail: `${t("mode.service_retry_failed")}: ${String(e)}`,
      life: 15000,
    });
  } finally {
    retryingService.value = false;
    await refreshServiceState();
  }
}

const aboutVisible = ref(false);
/** 启动/连接失败时保留的错误（顶部横幅展示，不再静默降级模式）。 */
const startupError = ref("");

/**
 * 端口冲突（`port_in_use`）：服务返回的结构化错误被人性化成
 * `RPC 端口 1995 被 Code.exe（pid=1234, kind=forwarded）占用`。
 * 这里解析出来渲染专用卡片（比一句红字有用得多）。
 */
const portConflict = ref<{ port: string; name: string; pid: string; kind: string } | null>(null);

function noteStartupError(e: unknown) {
  const text = String(e);
  startupError.value = text;
  const m = /RPC 端口 (\d+) 被 (.+?)（pid=(\d+), kind=(\w+)）占用/.exec(text);
  portConflict.value = m
    ? { port: m[1] ?? "", name: m[2] ?? "", pid: m[3] ?? "", kind: m[4] ?? "foreign" }
    : null;
}

/** 给用户的可复制命令（按平台）。 */
const conflictHint = computed(() => {
  const c = portConflict.value;
  if (!c) return "";
  if (c.kind === "forwarded")
    return t("port_conflict.hint_forwarded", { port: c.port, name: c.name });
  if (c.kind === "exvlan")
    return t("port_conflict.hint_exvlan", { port: c.port, name: c.name, pid: c.pid });
  return t("port_conflict.hint_foreign", { port: c.port, name: c.name });
});

async function copyConflictHint() {
  try {
    await navigator.clipboard.writeText(conflictHint.value);
    toast.add({ severity: "success", summary: t("port_conflict.copied"), life: 2000 });
  } catch (e: unknown) {
    toast.add({ severity: "error", summary: t("error"), detail: String(e), life: 8000 });
  }
}

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
    file_log_level: "info",
    // 端口：服务与普通模式都只用 1995（可在 service.toml 配置）；任何"保存过的
    // 自定义端口"都是 pre.12 的残留，照它连会连到没人监听的地址。
    rpc_portal: LOCAL_RPC_PORTAL,
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
    const state = await buildFrontendState().catch(() => "");
    const path = await shareErrorLog(state);
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
      rpc_portal: (currentMode.value as any).rpc_portal || "127.0.0.1:1995",
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
  } catch (e: unknown) {
    noteStartupError(e);
    toast.add({ severity: "error", summary: t("error"), detail: String(e), life: 10000 });
    return;
  }
  await initWithMode(currentMode.value);
}
const appUpdateDialogVisible = ref(false);
const appUpdateVersion = ref("");
/** 有可用更新（应用或内核）→ 设置齿轮上的小红点；不弹窗打扰。 */
const hasUpdateDot = ref(false);
const appUpdateNotes = ref("");
const appUpdating = ref(false);

const currentMode = ref<Mode>({ mode: "normal" });
const editingMode = ref<Mode>({ mode: "normal" });

/** 运行方式（本机内核）与目标（界面连哪里）互相正交。 */
const runtimeKind = ref<RuntimeKind>(loadRuntime());
const targetId = ref<string>(loadTargetId());

/** 两种运行方式共用的固定本地端口（服务与 supervisor 都写死这个值）。 */
const LOCAL_RPC_PORTAL = "127.0.0.1:1995";

const serviceDialogVisible = ref(false);
/** 配置里的网段变了但内核还没重启 → 远程管理对新网段暂不生效。 */
const whitelistNeedsRestart = ref(false);
const whitelistApplying = ref(false);

async function refreshWhitelistState() {
  if (type() === "android" || type() === "ios") return;
  try {
    const info = await serviceWhitelistInfo();
    whitelistNeedsRestart.value = !!info.needs_restart;
  } catch {
    whitelistNeedsRestart.value = false;
  }
}

async function onApplyWhitelist() {
  whitelistApplying.value = true;
  try {
    // 必须是「原子重启内核」，不能只重启服务本体：
    // 后者在 normal 策略下会把内核停掉却不拉起来（改配置就断网）。
    await serviceKernelRestart();
    // 内核进程换了，旧的 RPC 连接已失效 → 重新建连（不要留一个“假在线”的客户端）。
    await initWithMode(currentMode.value).catch((e: unknown) => {
      noteStartupError(e);
      void reportFrontendError("fe:kernel-restart", String(e));
    });
    await refreshWhitelistState();
    toast.add({
      severity: "success",
      summary: t("settings.privileged_service"),
      detail: t("service_settings.restarted"),
      life: 4000,
    });
  } catch (e: any) {
    toast.add({ severity: "error", summary: t("error"), detail: String(e), life: 12000 });
  } finally {
    whitelistApplying.value = false;
  }
}
type MigrateStatus = {
  pending: boolean;
  items: string[];
  tool: string | null;
  done: boolean;
};

/** 迁移闸门：非空时全屏阻塞，直到迁移完成 */
const migrateGate = ref<MigrateStatus | null>(null);
const migrateRunning = ref(false);
const migrateChecking = ref(false);
let migrateResolve: (() => void) | null = null;

/**
 * 安装包里没有 `exvlan-migrate`（下一版起默认如此）：用户自己跑独立工具后，
 * 这里重新检测一次；`pending=false` 就放行。
 */
async function onMigrateRecheck() {
  migrateChecking.value = true;
  try {
    const ms = await invoke<MigrateStatus>("migrate_status");
    if (!ms.pending) {
      migrateGate.value = null;
      migrateResolve?.();
      migrateResolve = null;
      return;
    }
    migrateGate.value = ms;
    toast.add({
      severity: "warn",
      summary: t("migrate.title"),
      detail: t("migrate.recheck_pending"),
      life: 8000,
    });
  } catch (e: unknown) {
    toast.add({ severity: "error", summary: t("error"), detail: String(e), life: 10000 });
  } finally {
    migrateChecking.value = false;
  }
}

async function onMigrateRun() {
  migrateRunning.value = true;
  try {
    await invoke<Record<string, unknown>>("migrate_run");
    toast.add({
      severity: "success",
      summary: t("migrate.title"),
      detail: t("migrate.done"),
      life: 4000,
    });
    migrateGate.value = null;
    migrateResolve?.();
    migrateResolve = null;
  } catch (e: unknown) {
    toast.add({ severity: "error", summary: t("error"), detail: String(e), life: 15000 });
    void reportFrontendError("fe:migrate", String(e));
  } finally {
    migrateRunning.value = false;
  }
}

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
    // 路由被拒绝时即使旧隧道还在跑也要提示，否则用户只在日志里看到原因。
    (!!mobileVpnRouteError.value || !mobileVpnRunning.value) &&
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
  const remotes = loadRemotes();
  const targetItems = [
    await MenuItem.new({
      id: `target-${LOCAL_TARGET}`,
      text: `${targetId.value === LOCAL_TARGET ? "✓ " : ""}${t("target.local")}`,
      action: async () => {
        await switchTarget(LOCAL_TARGET);
      },
    }),
    ...(await Promise.all(
      remotes.map((remote) =>
        MenuItem.new({
          id: `target-${remote.id}`,
          text: `${targetId.value === remote.id ? "✓ " : ""}${remote.name || remoteAddress(remote)}`,
          action: async () => {
            await switchTarget(remote.id);
          },
        }),
      ),
    )),
  ];
  const runtimeItems = await Promise.all(
    (["service", "normal", "off"] as RuntimeKind[]).map((kind) =>
      MenuItem.new({
        id: `runtime-${kind}`,
        text: `${runtimeKind.value === kind ? "✓ " : ""}${t(`runtime.${kind}`)}`,
        action: async () => {
          await switchRuntime(kind);
        },
      }),
    ),
  );
  await setTrayMenu([
    await MenuItemShow(t("tray.show")),
    await PredefinedMenuItem.new({ item: "Separator" }),
    await Submenu.new({ text: t("target.title"), items: targetItems }),
    await Submenu.new({ text: t("runtime.title"), items: runtimeItems }),
    await PredefinedMenuItem.new({ item: "Separator" }),
    await MenuItemExit(t("tray.exit")),
  ]);
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

async function initWithMode(rawMode: Mode) {
  const mode = await normalizeMode(rawMode);

  // 目标：本机 = 连本机内核；远程 = 连 mesh 里的另一台。
  // 远程时**本机内核照运行方式继续跑**（互不影响），但不再向远程推本地配置。
  const remote = findRemote(targetId.value);
  gatewayWritable.value = false;
  if (!remote && runtimeKind.value === "off") {
    // 运行方式=停止：本机没有 RPC 可连，界面保持「未运行」并提供重试入口。
    startupError.value = t("runtime.off_hint");
    clientRunning.value = false;
    return;
  }

  // 「经 gateway」的远端目标：控制面走对端 HTTP（Bearer），不拨内核 RPC。
  // 本机内核仍按 runtime 维护（互不影响），所以这里只探一次连通性。
  if (remote && remoteMode(remote) === "gateway") {
    void ensureRuntimeQuiet();
    try {
      await gatewaySystem(remote.id);
      // 权限来自 token 的 scopes：只读 token 就把新增/编辑入口一并藏掉。
      const paired = await gatewayRemotes();
      gatewayWritable.value = !!paired
        .find((item) => item.id === remote.id)
        ?.scopes?.includes("kernel:write");
    } catch (e) {
      startupError.value = t("target.gateway_unreachable", {
        addr: remoteAddress(remote),
        error: e instanceof Error ? e.message : String(e),
      });
      clientRunning.value = false;
      void reportFrontendError("fe:gateway-unreachable", startupError.value);
      return;
    }
    // 只读目标：不做「推本地配置」那套（那是写操作）。
    await afterConnect(mode, true);
    return;
  }
  const running_inst_ids = remote
    ? []
    : ((await remoteClient.value.list_network_instance_ids().catch(() => undefined))
        ?.running_inst_ids ?? []);

  // 新架构：service 与 normal 共用同一个常驻 root 守护进程，
  // 区别仅在于「退出应用时是否停实例」。因此切模式**不再**停/卸载服务
  // （旧逻辑会因状态不匹配而失败，且会破坏 normal 模式所需的 TUN）。

  let url: string | undefined = undefined;
  let retrys = 1;

  if (remote) {
    // 远程目标：本机内核仍按 runtime 维护（失败不阻塞远程连接，只记日志）。
    url = remoteUrl(remote);
    retrys = 3;
    void ensureRuntimeQuiet();
    await connectWithRetry(mode, url, retrys);

    // TCP 连上 ≠ 能管：对端内核的 RPC 白名单可能不含本机 —— 表现为
    // 连接建立但每个请求都 deadline，界面只会显示「无法连接」。
    // 这里探一次，把原因说清楚（最常见：对端还是 pre.12 的旧内核）。
    const manageable = await remoteClient.value
      .list_network_instance_ids()
      .then(() => true)
      .catch(() => false);
    if (!manageable) {
      startupError.value = t("target.remote_rejected", {
        addr: `${remote.host}:${DEFAULT_RPC_PORT}`,
      });
      clientRunning.value = false;
      void reportFrontendError("fe:remote-rejected", startupError.value);
      return;
    }

    await afterConnect(mode);
    return;
  }

  switch (mode.mode) {
    case "service": {
      // 端口与目录都是固定的，不再校验用户填的字段；ensureLocalKernel 幂等地
      // 完成「装服务 + 换内核」，版本一致时不会提权。
      mode.config_server_url = mode.config_server_url || undefined;
      await ensureLocalKernel(mode);
      url = "tcp://" + LOCAL_RPC_PORTAL;
      retrys = 5;
      break;
    }
    case "normal":
      url = "tcp://" + LOCAL_RPC_PORTAL;
      break;
  }
  await connectWithRetry(mode, url, retrys);
  await sendConfigs(running_inst_ids.map(Utils.UuidToStr));
  if (mode.mode === "normal") {
    mode.config_server_url = mode.config_server_url || undefined;
    initWebClient(mode.config_server_url);
  }
  await afterConnect(mode);
}

/** 建连（带重试）；失败时保留用户选择并抛错，由调用方决定怎么提示。 */
async function connectWithRetry(mode: Mode, url: string | undefined, retrys: number) {
  for (let i = 0; i < retrys; i++) {
    try {
      await connectRpcClient(mode.mode === "normal", url);
      return;
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
}

/** 连接成功后的收尾：记状态、恢复上次网络。 */
async function afterConnect(mode: Mode, running?: boolean) {
  currentMode.value = mode;
  startupError.value = "";
  // 经 gateway 的目标没有本机 RPC 连接，`isClientRunning()` 必然是 false，
  // 由调用方显式传 true（已探通对端）。
  clientRunning.value = running ?? (await isClientRunning());

  // 连接成功后再恢复一次上次网络（不依赖 clientRunning 的 false→true 翻转）
  // 注意：切模式后配置目录不同，上次的网络 ID 可能在新内核里不存在，
  // 直接使用会导致前端反复 `get_config` 失败（日志里的 `Not found: No such network instance`）。
  //
  // 换内核/重启服务后 RPC 会在最初几秒里 `deadline has elapsed`（实测 Windows 上
  // 连上后立刻查实例列表必然超时），所以这里退避重试，而不是一次就放弃。
  const lastId = await knownInstanceOrDropWithRetry(loadLastNetworkInstanceId());
  instanceId.value = lastId;
}

/** 换内核后实例列表可能短暂不可用：300/600/1200ms 退避重试。 */
async function knownInstanceOrDropWithRetry(
  candidate: string | null | undefined,
): Promise<string | undefined> {
  for (const wait of [0, 300, 600, 1200]) {
    if (wait > 0) {
      await new Promise((resolve) => setTimeout(resolve, wait));
    }
    try {
      return await knownInstanceOrDrop(candidate);
    } catch (e) {
      void logDiagStep("instance_restore_retry", String(e).slice(0, 120));
    }
  }
  return knownInstanceOrDrop(candidate);
}

/** 安静地应用当前运行方式（远程目标时用：失败只记日志，不影响远程连接）。 */
async function ensureRuntimeQuiet() {
  if (type() === "android" || type() === "ios") return;
  try {
    await runtimeApply(runtimeKind.value);
  } catch (e) {
    void logDiagStep("runtime_apply_failed", String(e).slice(0, 200));
  }
}

/** 切换目标：只换连接，不停本机内核。 */
async function switchTarget(id: string) {
  if (isModeSaving.value) return;
  if (id === targetId.value) return;
  targetId.value = id;
  await saveTargetId(id);
  isModeSaving.value = true;
  try {
    instanceId.value = undefined;
    await initWithMode(currentMode.value);
  } catch (e: any) {
    startupError.value = String(e);
    void reportFrontendError("fe:switch-target", String(e));
  } finally {
    isModeSaving.value = false;
    await refreshTrayMenu().catch(() => {});
  }
}

/** 切换运行方式（service / normal / off）。 */
async function switchRuntime(kind: RuntimeKind) {
  if (isModeSaving.value) return;
  if (kind === runtimeKind.value) return;
  isModeSaving.value = true;
  try {
    runtimeKind.value = kind;
    await saveRuntime(kind);
    if (kind === "off") {
      await runtimeApply("off");
      // 内核已停：本机连接必然失败，交给横幅提示，不再自动重连。
      clientRunning.value = false;
      startupError.value = t("runtime.off_hint");
      return;
    }
    const mode = await normalizeMode({ mode: kind } as unknown as Mode);
    currentMode.value = mode;
    await ensureLocalKernel(mode);
    await initWithMode(mode);
  } catch (e: any) {
    startupError.value = String(e);
    void reportFrontendError("fe:switch-runtime", String(e));
    toast.add({ severity: "error", summary: t("error"), detail: String(e), life: 10000 });
  } finally {
    isModeSaving.value = false;
    await refreshTrayMenu().catch(() => {});
  }
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

async function onServiceUninstalled() {
  // 服务已卸载：内核没人托管，按决策自动回落普通模式（本机子进程）。
  await switchRuntime("normal").catch(() => {});
  await refreshServiceState();
  await refreshTrayMenu().catch(() => {});
}

async function onServiceDialogChanged() {
  await refreshServiceState();
  await refreshTrayMenu().catch(() => {});
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
    rpc_portal: (mode as any).rpc_portal || saved.rpc_portal || "127.0.0.1:1995",
    file_log_level: (mode as any).file_log_level || saved.file_log_level || "info",
    file_log_dir: (mode as any).file_log_dir || saved.file_log_dir || defaults.file_log_dir,
    config_server: (mode as { config_server_url?: string }).config_server_url,
  };
  await ensureKernelServiceGuarded(merged);
}

/**
 * 启动静默检查（24h 节流）：
 * - 尊重设置里的「自动检查」（开关在 prefs）；
 * - 有更新只点小红点，不弹窗；
 * - 点过「跳过此版本」的不再提示（后端按通道记录）。
 */
async function maybeCheckAppUpdate() {
  const KEY_LAST = "upgrade_last_check";
  const last = Number(localStorage.getItem(KEY_LAST) || 0);
  if (Date.now() - last < 24 * 3600 * 1000) {
    return;
  }

  try {
    const prefs = await getUpdatePrefs();
    if (!prefs.auto_check) {
      return;
    }
    const info = await appUpdateInfo();
    localStorage.setItem(KEY_LAST, String(Date.now()));
    if (info.available && info.latest && prefs.skipped !== info.latest) {
      hasUpdateDot.value = true;
      appUpdateVersion.value = info.latest;
      appUpdateNotes.value = info.notes || "";
    }
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

  // 一次性迁移闸门（pre.17）：检测到旧数据就阻塞启动，等用户确认后跑随包工具。
  // 运行时不做任何迁移/兼容，迁移完成后才继续后面的初始化。
  const ms = await invoke<MigrateStatus>("migrate_status").catch(() => null);
  if (ms?.pending) {
    migrateGate.value = ms;
    await new Promise<void>((resolve) => {
      migrateResolve = resolve;
    });
  }

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

  // 运行方式 × 目标 是新的权威模型；旧 app_mode 仅在没有 runtime 时用于迁移。
  runtimeKind.value = loadRuntime();
  targetId.value = loadTargetId();
  // 首次从 app_mode 迁移时把结果写回 prefs.toml，避免两个来源长期漂移。
  void saveRuntime(runtimeKind.value).catch(() => {});
  void saveTargetId(targetId.value).catch(() => {});
  const hadAppMode = localStorage.getItem("app_mode") !== null;
  const savedMode = loadMode();
  // 本机运行方式由 runtimeKind 决定（app_mode 里的 remote 已在 target 迁移中处理）。
  const derivedMode = (
    savedMode.mode === "service" || savedMode.mode === "normal"
      ? { ...savedMode, mode: runtimeKind.value === "off" ? "service" : runtimeKind.value }
      : { mode: runtimeKind.value === "off" ? "service" : runtimeKind.value }
  ) as Mode;
  currentMode.value = await normalizeMode(derivedMode);
  // 运行方式是唯一权威（prefs 的 runtime）：`app_mode` 只读不写，仅用于旧数据迁移。
  const modeSource = hadAppMode ? "persisted" : "default";
  void reportBootState(currentMode.value.mode, loadLastNetworkInstanceId(), null, modeSource);

  // 首次安装/升级后会自动装好内核并（重新）注册系统服务；失败不阻塞进入界面。
  // service 模式由 initWithMode 内部统一处理，避免同一次启动弹两次授权。
  if (currentMode.value.mode !== "service" && runtimeKind.value !== "off") {
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
  void refreshServiceState();

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

/** 当前目标（`undefined` = 本机）。 */
const currentRemote = computed(() => findRemote(targetId.value));
/** 「经 gateway」的只读目标：控制面走 HTTP + Bearer，不拨内核 RPC。 */
const isGatewayTarget = computed(
  () => !!currentRemote.value && remoteMode(currentRemote.value) === "gateway",
);
const remoteClient = computed(() =>
  isGatewayTarget.value && currentRemote.value
    ? new GatewayRemoteClient(currentRemote.value.id)
    : new GUIRemoteClient(),
);
/** 对端 token 是否带写权限（配对时选了 config/admin 才有）。 */
const gatewayWritable = ref(false);
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
  // 只重连，不改任何模式状态（运行方式由状态栏/托盘切换）。
  startupError.value = "";
  await initWithMode(currentMode.value);
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
async function onConfigServerSave(event?: MouseEvent) {
  if (JSON.stringify(currentMode.value) === JSON.stringify(editingMode.value)) {
    configServerDialogVisible.value = false;
    return;
  }
  if (editingMode.value.mode === "service") {
    await new Promise<void>((resolve, reject) => {
      confirm.require({
        // 锚到触发按钮（按钮旁弹出），没有事件时退回页面根节点。
        target:
          (event?.currentTarget as HTMLElement | null) ??
          (document.querySelector("#root") as HTMLElement | null) ??
          document.body,
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
  currentMode.value = {
    ...currentMode.value,
    config_server_url: (editingMode.value as WebClientConfig).config_server_url,
  } as Mode;
  await initWithMode(currentMode.value);
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
          @click="onConfigServerSave($event)"
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

    <!-- 端口冲突（port_in_use）：给出「谁占的 + 怎么办」，而不是一句报错 -->
    <div
      v-if="portConflict"
      class="mb-2 rounded border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-900 dark:border-amber-700 dark:bg-amber-950 dark:text-amber-100"
    >
      <div class="flex items-center gap-2 font-medium">
        <i class="pi pi-exclamation-circle" />
        <span>{{ t("port_conflict.title", { port: portConflict.port }) }}</span>
        <span class="text-xs opacity-70">
          {{ portConflict.name }} · pid={{ portConflict.pid }} · {{ portConflict.kind }}
        </span>
      </div>
      <p class="mt-1 whitespace-pre-line text-xs leading-5">{{ conflictHint }}</p>
      <div class="mt-2 flex flex-wrap gap-2">
        <Button
          size="small"
          :label="t('port_conflict.copy_cmd')"
          icon="pi pi-copy"
          @click="copyConflictHint"
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
          :label="t('client.retry_startup')"
          icon="pi pi-replay"
          @click="retryStartup"
        />
      </div>
    </div>

    <!-- 网段改了但内核还没重启：远程管理对新网段暂不生效 -->
    <div
      v-if="whitelistNeedsRestart"
      class="flex items-center gap-2 border-l-4 border-sky-500 bg-sky-50 px-3 py-2 text-sm text-sky-900 dark:bg-sky-950 dark:text-sky-200"
    >
      <i class="pi pi-info-circle" />
      <span class="flex-1">{{ t("service_settings.needs_restart_banner") }}</span>
      <Button
        size="small"
        :label="t('service_settings.apply_and_restart')"
        :loading="whitelistApplying"
        @click="onApplyWhitelist"
      />
    </div>

    <div
      v-if="!serviceReady"
      class="flex items-center gap-2 border-l-4 border-amber-500 bg-amber-50 px-3 py-2 text-sm text-amber-900 dark:bg-amber-950 dark:text-amber-200"
    >
      <i class="pi pi-shield" />
      <span class="flex-1 break-all">
        {{ t("mode.service_missing") }}
        <span v-if="serviceStatusText" class="opacity-70">({{ serviceStatusText }})</span>
      </span>
      <Button
        size="small"
        :label="t('mode.service_retry')"
        icon="pi pi-refresh"
        :loading="retryingService"
        :disabled="retryingService"
        @click="onRetryService(false)"
      />
    </div>

    <div
      v-if="showVpnNotRunningBanner"
      class="flex items-center gap-2 border-l-4 border-amber-500 bg-amber-50 px-3 py-2 text-sm text-amber-900 dark:bg-amber-950 dark:text-amber-200"
    >
      <i class="pi pi-exclamation-circle" />
      <span v-if="mobileVpnRouteError" class="flex-1 break-all">
        {{ mobileVpnRouteError }}
      </span>
      <span v-else class="flex-1">{{
        mobileVpnPermissionDenied
          ? t("settings.vpn_need_permission")
          : t("settings.vpn_not_connected_hint")
      }}</span>
      <Button
        v-if="!mobileVpnRouteError"
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
      :readonly="isGatewayTarget && !gatewayWritable"
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
      <TargetSwitcher
        :runtime="runtimeKind"
        :target-id="targetId"
        :disabled="isModeSaving"
        @switch-runtime="switchRuntime"
        @switch-target="switchTarget"
        @remotes-changed="refreshTrayMenu()"
      />
      <span class="flex-1" />
      <Button
        icon="pi pi-cog"
        text
        rounded
        class="relative"
        :aria-label="t('settings.title')"
        @click="
          settingsVisible = true;
          hasUpdateDot = false;
        "
      >
        <span v-if="hasUpdateDot" class="absolute right-1 top-1 h-2 w-2 rounded-full bg-red-500" />
      </Button>
      <Button
        icon="pi pi-power-off"
        text
        rounded
        severity="danger"
        :aria-label="t('exit')"
        @click="exit(1)"
      />
    </div>

    <ServiceSettings
      v-model:visible="serviceDialogVisible"
      @changed="onServiceDialogChanged"
      @uninstalled="onServiceUninstalled"
    />

    <SettingsDialog
      :remote-target="isGatewayTarget"
      v-model:visible="settingsVisible"
      :initial-section="settingsSection"
      :mode="currentMode"
      :client-running="clientRunning"
      :service-ready="serviceReady"
      :config-server-status="configServerConnectionStatus"
      :log-level="logLevel"
      @open-service="serviceDialogVisible = true"
      @open-config-server="openConfigServerDialog"
      @open-log-dir="onOpenLogDir"
      @copy-log-dir="onCopyLogDir"
      @share-log="onShareErrorLog"
      @set-log-level="onSetLogLevel"
      @changed-language="refreshTrayMenu"
    />

    <!-- 一次性迁移（pre.17）：阻塞启动，完成后自动继续 -->
    <div
      v-if="migrateGate"
      class="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4"
    >
      <div class="w-full max-w-xl rounded-lg bg-surface-0 p-6 shadow-xl dark:bg-surface-900">
        <div class="mb-3 text-lg font-semibold">{{ t("migrate.title") }}</div>
        <p class="mb-3 text-sm text-surface-600 dark:text-surface-300">
          {{ t("migrate.hint") }}
        </p>
        <!-- 下一版起安装包不再随带迁移工具：此时必须给出「自己跑工具 + 重试」
             的路径，而不是让「开始迁移」按钮点出一个错误。 -->
        <p
          v-if="!migrateGate.tool"
          class="mb-3 rounded bg-amber-50 p-3 text-sm text-amber-800 dark:bg-amber-900/30 dark:text-amber-200"
        >
          {{ t("migrate.no_tool_hint") }}
        </p>
        <ul
          class="mb-4 max-h-48 overflow-auto rounded bg-surface-100 p-3 text-xs font-mono dark:bg-surface-800"
        >
          <li v-for="item in migrateGate.items" :key="item" class="break-all">{{ item }}</li>
        </ul>
        <div class="flex items-center justify-end gap-2">
          <Button
            :label="t('migrate.exit')"
            severity="secondary"
            text
            :disabled="migrateRunning"
            @click="exit(1)"
          />
          <Button
            v-if="migrateGate.tool"
            :label="migrateRunning ? t('migrate.running') : t('migrate.run')"
            :loading="migrateRunning"
            @click="onMigrateRun"
          />
          <Button
            v-else
            :label="t('migrate.recheck')"
            :loading="migrateChecking"
            @click="onMigrateRecheck"
          />
        </div>
      </div>
    </div>
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
