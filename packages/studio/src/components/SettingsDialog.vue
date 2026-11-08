<script setup lang="ts">
import { type } from "@tauri-apps/plugin-os";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { open } from "@tauri-apps/plugin-shell";
import { I18nUtils, Utils } from "@exvlan/std";
import { useToast } from "openvue";
import type { Mode } from "~/composables/mode";
import About from "~/components/About.vue";
import KernelManager from "~/components/KernelManager.vue";
import {
  getConfig,
  listNetworkInstanceIds,
  networkDiagnostics,
  serviceDiagnostics,
  serviceReinstall,
  serviceRestart,
  uploadDiagnostics,
} from "~/composables/backend";
import { mobileVpnDiagnostics } from "~/composables/mobile_vpn";

const props = defineProps<{
  visible: boolean;
  mode: Mode;
  clientRunning: boolean;
  configServerStatus: string;
  logLevel: string;
  initialSection?: Section;
}>();

const emit = defineEmits<{
  (e: "update:visible", value: boolean): void;
  (e: "open-mode"): void;
  (e: "open-autostart"): void;
  (e: "open-config-server"): void;
  (e: "open-log-dir"): void;
  (e: "copy-log-dir"): void;
  (e: "share-log"): void;
  (e: "set-log-level", level: string): void;
  (e: "changed-language"): void;
}>();

const { t, locale } = useI18n();
const toast = useToast();

const platform = type();
const isMobile = platform === "android" || platform === "ios";
const isDesktop = !isMobile;

type Section = "general" | "network" | "logging" | "about";
const section = ref<Section>(props.initialSection ?? "general");

watch(
  () => props.initialSection,
  (next) => {
    if (next) {
      section.value = next;
    }
  },
);

const sections = computed(() => [
  { key: "general" as const, label: t("settings.general"), icon: "pi pi-cog" },
  { key: "network" as const, label: t("settings.network"), icon: "pi pi-globe" },
  { key: "logging" as const, label: t("settings.logging"), icon: "pi pi-file" },
  { key: "about" as const, label: t("settings.about"), icon: "pi pi-info-circle" },
]);

const logLevels = ["off", "warn", "info", "debug", "trace"];
const logLevelLabel = (level: string) => t(`logging_level_${level}`);

const modeLabel = computed(() => t(`mode.${props.mode.mode}`));

const configServerLabel = computed(() => {
  if (!["normal", "service"].includes(props.mode.mode)) {
    return "-";
  }
  return `${t("config-server.title")}${t(`config-server.${props.configServerStatus}`)}`;
});

/* ---------------- 服务自检 ---------------- */
const checking = ref(false);
const restarting = ref(false);
const reinstalling = ref(false);
const diagnostics = ref("");

async function runDiagnostics() {
  checking.value = true;
  try {
    diagnostics.value = await serviceDiagnostics();
  } catch (e: any) {
    diagnostics.value = String(e);
  } finally {
    checking.value = false;
  }
}

async function restartService() {
  restarting.value = true;
  try {
    await serviceRestart();
    toast.add({
      severity: "success",
      summary: t("web.common.success"),
      detail: t("settings.service_restart"),
      life: 3000,
    });
    await runDiagnostics();
  } catch (e: any) {
    toast.add({ severity: "error", summary: t("error"), detail: String(e), life: 10000 });
  } finally {
    restarting.value = false;
  }
}

async function reinstallService() {
  reinstalling.value = true;
  try {
    await serviceReinstall({
      config_dir: "",
      rpc_portal: (props.mode as any).rpc_portal || "127.0.0.1:15999",
      file_log_level: (props.mode as any).file_log_level || "info",
      file_log_dir: "",
      config_server: (props.mode as any).config_server_url,
    });
    toast.add({
      severity: "success",
      summary: t("web.common.success"),
      detail: t("settings.service_reinstall"),
      life: 3000,
    });
    await runDiagnostics();
  } catch (e: any) {
    toast.add({ severity: "error", summary: t("error"), detail: String(e), life: 10000 });
  } finally {
    reinstalling.value = false;
  }
}

async function copyDiagnostics() {
  await writeText(diagnostics.value);
  toast.add({
    severity: "success",
    summary: t("web.common.success"),
    detail: t("settings.copy"),
    life: 2000,
  });
}

/* ---------------- 通用 ---------------- */
async function switchLanguage() {
  await I18nUtils.loadLanguageAsync(locale.value === "en" ? "cn" : "en");
  emit("changed-language");
}

async function openProjectHome() {
  try {
    await open("https://github.com/exvlan/exvlan");
  } catch (e: any) {
    toast.add({ severity: "error", summary: t("error"), detail: String(e), life: 6000 });
  }
}

const LAST_UPLOAD_KEY = "diag_last_upload";

/** 前端才有的关键信息：配置摘要 + VPN 运行时状态（实际下发的 routes / dns / 所属实例）。 */
async function buildFrontendState(): Promise<string> {
  // 桌面端没有 VpnService，VPN 段由 Rust 侧输出 n/a，这里不再重复。
  const lines = isMobile ? [mobileVpnDiagnostics()] : [];
  try {
    const ids = await listNetworkInstanceIds();
    const list = [...(ids.running_inst_ids ?? []), ...(ids.disabled_inst_ids ?? [])]
      .map((id) => (typeof id === "string" ? id : Utils.UuidToStr(id as any)))
      .filter((id) => id.length > 0);
    lines.push(`--- config (${list.length}) ---`);
    for (const id of list) {
      const cfg = await getConfig(id).catch(() => null);
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

const netChecking = ref(false);
const netReport = ref("");

async function copyNetworkReport() {
  if (!netReport.value) {
    return;
  }
  await writeText(netReport.value);
  toast.add({
    severity: "success",
    summary: t("web.common.success"),
    detail: t("settings.copy"),
    life: 2000,
  });
}

async function runNetworkCheck() {
  netChecking.value = true;
  try {
    netReport.value = await networkDiagnostics(await buildFrontendState());
  } catch (e: any) {
    netReport.value = String(e);
  } finally {
    netChecking.value = false;
  }
}

const uploading = ref(false);
const CONSENT_KEY = "diag_upload_consent";
const consentVisible = ref(false);
let consentResolve: ((ok: boolean) => void) | null = null;

/**
 * 首次上传前的确认。
 *
 * 不能用 `window.confirm`：macOS 的 WKWebView（wry）没有实现
 * `runJavaScriptConfirmPanelWithMessage`，调用会**同步返回 false 且不显示任何界面**，
 * 表现为「点上传完全没反应、连 loading 都没有」。
 */
function askUploadConsent(): Promise<boolean> {
  if (localStorage.getItem(CONSENT_KEY) === "1") {
    return Promise.resolve(true);
  }
  consentVisible.value = true;
  return new Promise<boolean>((resolve) => {
    consentResolve = resolve;
  });
}

function resolveUploadConsent(ok: boolean) {
  consentVisible.value = false;
  if (ok) {
    localStorage.setItem(CONSENT_KEY, "1");
  }
  consentResolve?.(ok);
  consentResolve = null;
}

async function onUploadDiagnostics() {
  if (!(await askUploadConsent())) {
    return;
  }
  uploading.value = true;
  try {
    const urls = await uploadDiagnostics(await buildFrontendState());
    if (urls) {
      localStorage.setItem(LAST_UPLOAD_KEY, JSON.stringify({ at: new Date().toISOString(), urls }));
      await writeText(urls);
      toast.add({
        severity: "success",
        summary: t("settings.upload_diag"),
        detail: `已复制链接：${urls}`,
        life: 12000,
      });
    }
  } catch (e: any) {
    console.warn("upload diagnostics failed, fallback to share", e);
    toast.add({
      severity: "warn",
      summary: t("settings.upload_diag"),
      detail: `上传失败（${String(e)}），改用分享`,
      life: 8000,
    });
    emit("share-log");
  } finally {
    uploading.value = false;
  }
}

watch(
  () => props.visible,
  (visible) => {
    if (visible && section.value === "network") {
      void runDiagnostics();
    }
  },
);
</script>

<template>
  <Dialog
    :visible="visible"
    modal
    :header="t('settings.title')"
    :style="
      isMobile
        ? {
            width: '100vw',
            height: '100vh',
            maxWidth: '100vw',
            maxHeight: '100vh',
            margin: 0,
            borderRadius: 0,
          }
        : { width: '900px', maxWidth: '96vw' }
    "
    @update:visible="emit('update:visible', $event)"
  >
    <div
      :class="isMobile ? 'flex flex-col' : 'flex'"
      :style="isMobile ? {} : { minHeight: '460px' }"
    >
      <!-- 左导航（微信式） -->
      <!-- 移动端：顶部横向标签 -->
      <nav v-if="isMobile" class="flex gap-1 mb-3 overflow-x-auto shrink-0">
        <button
          v-for="item in sections"
          :key="item.key"
          type="button"
          class="flex items-center gap-1 rounded-full px-3 py-1.5 whitespace-nowrap text-sm"
          :class="
            section === item.key
              ? 'bg-emerald-500 text-white'
              : 'bg-surface-100 dark:bg-surface-800'
          "
          @click="
            section = item.key;
            item.key === 'network' && runDiagnostics();
          "
        >
          <i :class="item.icon" />
          <span>{{ item.label }}</span>
        </button>
      </nav>

      <!-- 桌面：左侧导航 -->
      <nav
        v-else
        class="flex flex-col gap-1 pr-3 border-r border-surface-200 dark:border-surface-700"
        style="width: 180px"
      >
        <button
          v-for="item in sections"
          :key="item.key"
          type="button"
          class="flex items-center gap-2 rounded px-3 py-2 text-left transition-colors"
          :class="
            section === item.key
              ? 'bg-emerald-500 text-white'
              : 'hover:bg-surface-100 dark:hover:bg-surface-800'
          "
          @click="
            section = item.key;
            item.key === 'network' && runDiagnostics();
          "
        >
          <i :class="item.icon" />
          <span class="flex-1">{{ item.label }}</span>
        </button>
      </nav>

      <!-- 右侧内容 -->
      <div
        class="flex-1 overflow-y-auto"
        :class="isMobile ? '' : 'pl-6'"
        :style="isMobile ? {} : { maxHeight: '460px' }"
      >
        <!-- 通用 -->
        <div v-if="section === 'general'" class="flex flex-col">
          <div class="settings-row">
            <div class="flex-1">
              <div class="font-medium">{{ t("settings.language") }}</div>
            </div>
            <Button
              :label="locale === 'en' ? '中文' : 'English'"
              size="small"
              severity="secondary"
              @click="switchLanguage"
            />
          </div>

          <div class="settings-row">
            <div class="flex-1">
              <div class="font-medium">{{ t("settings.mode") }}</div>
              <div class="text-sm text-surface-500">{{ modeLabel }}</div>
            </div>
            <Button
              v-if="isDesktop"
              :label="t('web.common.edit')"
              size="small"
              severity="secondary"
              @click="emit('open-mode')"
            />
          </div>

          <div v-if="isDesktop" class="settings-row">
            <div class="flex-1">
              <div class="font-medium">{{ t("settings.autostart") }}</div>
            </div>
            <Button
              :label="t('web.common.edit')"
              size="small"
              severity="secondary"
              @click="emit('open-autostart')"
            />
          </div>
        </div>

        <!-- 网络 -->
        <div v-else-if="section === 'network'" class="flex flex-col">
          <div class="settings-row">
            <div class="flex-1">
              <div class="font-medium">{{ t("settings.config_server") }}</div>
              <div class="text-sm text-surface-500">{{ configServerLabel }}</div>
            </div>
            <Button
              v-if="['normal', 'service'].includes(mode.mode)"
              :label="t('web.common.edit')"
              size="small"
              severity="secondary"
              @click="emit('open-config-server')"
            />
          </div>

          <div class="settings-row">
            <div class="flex-1">
              <div class="font-medium">{{ t("settings.kernel") }}</div>
              <div class="text-sm text-surface-500">
                {{ clientRunning ? t("settings.rpc_connected") : t("settings.rpc_disconnected") }}
              </div>
            </div>
            <Button
              :label="t('settings.service_check_run')"
              size="small"
              severity="secondary"
              :loading="checking"
              @click="runDiagnostics"
            />
          </div>

          <div class="settings-row">
            <div class="flex-1">
              <div class="font-medium">{{ t("settings.network_check") }}</div>
              <div class="text-sm text-surface-500">{{ t("settings.network_check_hint") }}</div>
            </div>
            <Button
              :label="t('settings.network_check_run')"
              size="small"
              severity="secondary"
              :loading="netChecking"
              @click="runNetworkCheck"
            />
            <Button
              :label="t('settings.copy')"
              size="small"
              text
              icon="pi pi-copy"
              :disabled="!netReport"
              @click="copyNetworkReport"
            />
          </div>
          <pre
            v-if="netReport"
            class="text-xs whitespace-pre-wrap break-all bg-surface-100 dark:bg-surface-800 rounded p-2 max-h-64 overflow-y-auto"
            >{{ netReport }}</pre>

          <KernelManager v-if="isDesktop" class="mt-2" />

          <div v-if="isDesktop" class="mt-4 flex flex-col gap-2">
            <div class="flex items-center gap-2">
              <span class="font-medium">{{ t("settings.service_check") }}</span>
              <span class="flex-1" />
              <Button
                :label="t('settings.service_restart')"
                size="small"
                severity="secondary"
                :loading="restarting"
                @click="restartService"
              />
              <Button
                :label="t('settings.service_reinstall')"
                size="small"
                severity="danger"
                outlined
                :loading="reinstalling"
                @click="reinstallService"
              />
              <Button
                :label="t('settings.copy')"
                size="small"
                text
                icon="pi pi-copy"
                @click="copyDiagnostics"
              />
            </div>
            <pre
              class="text-xs whitespace-pre-wrap break-all bg-surface-100 dark:bg-surface-800 rounded p-2 max-h-56 overflow-y-auto"
              >{{ diagnostics || "-" }}</pre>
          </div>
        </div>

        <!-- 日志 -->
        <div v-else-if="section === 'logging'" class="flex flex-col">
          <div class="settings-row">
            <div class="flex-1">
              <div class="font-medium">{{ t("settings.log_level") }}</div>
            </div>
            <Select
              :model-value="logLevel"
              :options="logLevels"
              size="small"
              style="min-width: 140px"
              @update:model-value="(v: any) => emit('set-log-level', String(v))"
            >
              <template #value="{ value }">
                {{ logLevelLabel(value) }}
              </template>
              <template #option="{ option }">
                {{ logLevelLabel(option) }}
              </template>
            </Select>
          </div>

          <div v-if="isDesktop" class="settings-row">
            <div class="flex-1">
              <div class="font-medium">{{ t("settings.open_log_dir") }}</div>
            </div>
            <Button
              :label="t('settings.open_log_dir')"
              size="small"
              severity="secondary"
              icon="pi pi-folder-open"
              @click="emit('open-log-dir')"
            />
          </div>

          <div class="settings-row">
            <div class="flex-1">
              <div class="font-medium">{{ t("settings.copy_log_dir") }}</div>
            </div>
            <Button
              :label="t('settings.copy')"
              size="small"
              severity="secondary"
              icon="pi pi-copy"
              @click="emit('copy-log-dir')"
            />
          </div>

          <div class="settings-row">
            <div class="flex-1">
              <div class="font-medium">{{ t("settings.share_log") }}</div>
            </div>
            <Button
              :label="t('settings.share_log')"
              size="small"
              severity="secondary"
              icon="pi pi-share-alt"
              @click="emit('share-log')"
            />
          </div>

          <div class="settings-row">
            <div class="flex-1">
              <div class="font-medium">{{ t("settings.upload_diag") }}</div>
              <div class="text-sm text-surface-500">{{ t("settings.upload_diag_hint") }}</div>
            </div>
            <Button
              :label="t('settings.upload_diag')"
              size="small"
              icon="pi pi-cloud-upload"
              :loading="uploading"
              @click="onUploadDiagnostics"
            />
          </div>
        </div>

        <!-- 关于 -->
        <div v-else class="flex flex-col">
          <About />
        </div>
      </div>
    </div>

    <template #footer>
      <div class="w-full flex flex-col items-center gap-1 py-1">
        <div class="flex items-center gap-4 text-sm text-surface-500">
          <a class="cursor-pointer hover:underline" @click="emit('share-log')">{{
            t("settings.share_log")
          }}</a>
          <span class="opacity-40">|</span>
          <a class="cursor-pointer hover:underline" @click="openProjectHome">{{
            t("settings.project")
          }}</a>
          <span class="opacity-40">|</span>
          <a class="cursor-pointer hover:underline" @click="section = 'about'">{{
            t("settings.about")
          }}</a>
        </div>
        <div class="text-xs text-surface-400">{{ t("settings.copyright") }}</div>
      </div>
    </template>
  </Dialog>

  <!-- 诊断上传首次确认：应用内对话框（window.confirm 在 macOS WKWebView 中无效） -->
  <Dialog
    v-model:visible="consentVisible"
    modal
    :header="t('settings.upload_diag')"
    :style="{ width: '30rem', maxWidth: '92vw' }"
  >
    <p class="text-sm leading-relaxed whitespace-pre-line">{{ t("settings.upload_consent") }}</p>
    <template #footer>
      <Button
        :label="t('web.common.cancel')"
        severity="secondary"
        text
        @click="resolveUploadConsent(false)"
      />
      <Button :label="t('web.common.confirm')" @click="resolveUploadConsent(true)" />
    </template>
  </Dialog>
</template>

<style scoped>
.settings-row {
  display: flex;
  align-items: center;
  gap: 1rem;
  padding: 0.9rem 0.25rem;
  border-bottom: 1px solid var(--p-surface-200, #e5e7eb);
}
.settings-row:last-child {
  border-bottom: none;
}
</style>
