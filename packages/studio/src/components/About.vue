<script setup lang="ts">
import pkg from "~/../package.json";
import { getExvlanVersion } from "~/composables/backend";
import {
  appUpdateApply,
  appUpdateInfo,
  type AppUpdateInfo,
  type DiagnosticsInfo,
  diagnosticsInfo,
  shareErrorLog,
} from "~/composables/backend";
import { useToast } from "openvue";
import { type } from "@tauri-apps/plugin-os";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import KernelManager from "~/components/KernelManager.vue";

const { t } = useI18n();
const toast = useToast();

const appVersion = ref(pkg.version);
const kernelVersion = ref("");
const appInfo = ref<AppUpdateInfo | null>(null);
const checking = ref(false);
const downloading = ref(false);

const isDesktop = computed(() => type() !== "android" && type() !== "ios");

const diagnostics = ref<DiagnosticsInfo | null>(null);
const diagnosticsText = computed(() => {
  const d = diagnostics.value;
  if (!d) {
    return "";
  }
  const lines = [
    `version: ${d.version}`,
    `os: ${d.os} (${d.arch})`,
    `home: ${d.home}`,
    `log_dir: ${d.log_dir ?? "-"} (${d.log_dir_writable ? "writable" : "NOT writable"})`,
    `config_dir: ${d.config_dir ?? "-"} (${d.config_dir_writable ? "writable" : "NOT writable"})`,
    `error_log: ${d.error_log ?? "-"} (${d.error_log_size ?? 0} bytes)`,
    `kernel_installed: ${d.kernel_installed ?? "-"}`,
    `kernel_version: ${d.kernel_version ?? "-"}`,
    `service_status: ${d.service_status ?? "-"}`,
    `service_config_dir: ${d.service_config_dir ?? "-"}`,
    `service_log_dir: ${d.service_log_dir ?? "-"}`,
  ];
  return lines.join("\n");
});

async function loadDiagnostics() {
  diagnostics.value = await diagnosticsInfo().catch(() => null);
  // 应用版本优先取 Rust 侧的构建版本串（含预览渠道与 short sha），
  // package.json 的 version 只是标准版本，无法区分预览构建。
  if (diagnostics.value?.version) {
    appVersion.value = diagnostics.value.version;
  }
}

/* ---------------- 最近一次诊断上传（设置页写 localStorage，这里展示） ---------------- */
const LAST_UPLOAD_KEY = "diag_last_upload";
const lastUpload = ref<{ at: string; urls: string } | null>(null);

function loadLastUpload() {
  try {
    lastUpload.value = JSON.parse(localStorage.getItem(LAST_UPLOAD_KEY) || "null");
  } catch {
    lastUpload.value = null;
  }
}

async function copyLastUpload() {
  if (!lastUpload.value?.urls) {
    return;
  }
  await writeText(lastUpload.value.urls);
  toast.add({
    severity: "success",
    summary: t("web.common.success"),
    detail: t("about.copy_diagnostics"),
    life: 3000,
  });
}

async function copyDiagnostics() {
  await loadDiagnostics();
  if (!diagnosticsText.value) {
    return;
  }
  await writeText(diagnosticsText.value);
  toast.add({
    severity: "success",
    summary: t("web.common.success"),
    detail: t("about.copy_diagnostics"),
    life: 3000,
  });
}

async function shareLog() {
  try {
    const path = await shareErrorLog();
    if (path.startsWith("saved: ")) {
      toast.add({
        severity: "warn",
        summary: t("about.error_log"),
        detail: `分享不可用，已保存到 ${path.slice(7)}`,
        life: 15000,
      });
      return;
    }
    if (!isDesktop.value) {
      return;
    }
    await writeText(path);
    toast.add({ severity: "info", summary: t("about.error_log"), detail: path, life: 6000 });
  } catch (e: any) {
    toast.add({ severity: "error", summary: t("error"), detail: String(e), life: 8000 });
  }
}

async function loadKernelVersion() {
  kernelVersion.value = await getExvlanVersion().catch(() => "");
}

onMounted(() => {
  void loadKernelVersion();
  void loadDiagnostics();
  loadLastUpload();
});

async function onCheckApp() {
  checking.value = true;
  try {
    appInfo.value = await appUpdateInfo();
    if (!appInfo.value.available) {
      toast.add({ severity: "info", summary: t("upgrade.app_up_to_date"), life: 3000 });
    }
  } catch (e: any) {
    toast.add({ severity: "error", summary: t("error"), detail: String(e), life: 8000 });
  } finally {
    checking.value = false;
  }
}

async function onDownloadApp() {
  const version = appInfo.value?.latest;
  if (!version) {
    return;
  }
  downloading.value = true;
  try {
    await appUpdateApply(version);
    toast.add({
      severity: "success",
      summary: t("upgrade.downloaded"),
      detail: version,
      life: 8000,
    });
  } catch (e: any) {
    toast.add({ severity: "error", summary: t("error"), detail: String(e), life: 10000 });
  } finally {
    downloading.value = false;
  }
}
</script>

<template>
  <Card>
    <template #title> Exvlan </template>
    <template #content>
      <p class="mb-3">
        {{ t("about.description") }}
      </p>

      <!-- 应用版本 -->
      <div class="flex flex-wrap items-center gap-3 mb-2">
        <span class="font-medium">{{ t("upgrade.app_version") }}:</span>
        <span>{{ appVersion }}</span>
        <Button
          :label="t('upgrade.check')"
          size="small"
          severity="secondary"
          :loading="checking"
          @click="onCheckApp"
        />
        <template v-if="appInfo">
          <span v-if="appInfo.available" class="text-sm text-primary">
            {{ t("upgrade.new_version", { version: appInfo.latest }) }}
          </span>
          <span v-else class="text-sm text-green-600">{{ t("upgrade.app_up_to_date") }}</span>
        </template>
      </div>
      <div v-if="appInfo?.available" class="flex flex-wrap items-center gap-3 mb-3">
        <Button
          :label="t('upgrade.download_and_install')"
          size="small"
          :loading="downloading"
          @click="onDownloadApp"
        />
        <span v-if="appInfo.notes" class="text-sm text-gray-500 whitespace-pre-wrap">{{
          appInfo.notes
        }}</span>
      </div>

      <!-- 内核版本 / 内核更新（桌面：更新源与在线升级都在这里） -->
      <div class="flex flex-wrap items-center gap-3">
        <span class="font-medium">{{ t("upgrade.kernel_version") }}:</span>
        <span>{{ kernelVersion || t("upgrade.unknown") }}</span>
      </div>
      <KernelManager
        v-if="isDesktop"
        class="mt-4"
        :show-current="false"
        @updated="loadKernelVersion"
      />

      <!-- 诊断信息：路径 / 可写性 / 服务状态（排障时一键复制） -->
      <hr class="my-4 border-t border-surface-200 dark:border-surface-700" />
      <div class="flex flex-wrap items-center gap-2 mb-2">
        <span class="font-medium">{{ t("about.diagnostics") }}</span>
        <Button
          :label="t('about.copy_diagnostics')"
          size="small"
          severity="secondary"
          icon="pi pi-copy"
          @click="copyDiagnostics"
        />
        <Button
          :label="t('client.share_error_log')"
          size="small"
          text
          icon="pi pi-share-alt"
          @click="shareLog"
        />
      </div>
      <div v-if="diagnostics" class="text-xs font-mono whitespace-pre-wrap break-all leading-5">
        <div>{{ t("about.home") }}: {{ diagnostics.home }}</div>
        <div>
          {{ t("about.log_dir") }}: {{ diagnostics.log_dir || "-" }}
          <span :class="diagnostics.log_dir_writable ? 'text-green-600' : 'text-red-600'">
            ({{ diagnostics.log_dir_writable ? t("about.writable") : t("about.not_writable") }})
          </span>
        </div>
        <div>
          {{ t("about.config_dir") }}: {{ diagnostics.config_dir || "-" }}
          <span :class="diagnostics.config_dir_writable ? 'text-green-600' : 'text-red-600'">
            ({{ diagnostics.config_dir_writable ? t("about.writable") : t("about.not_writable") }})
          </span>
        </div>
        <div>
          {{ t("about.error_log") }}: {{ diagnostics.error_log || "-" }} ({{
            diagnostics.error_log_size ?? 0
          }}
          B)
        </div>
        <div v-if="diagnostics.service_status">
          {{ t("about.service_status") }}: {{ diagnostics.service_status }}
        </div>
        <div v-if="diagnostics.service_config_dir">
          service config: {{ diagnostics.service_config_dir }}
        </div>
        <div v-if="diagnostics.service_log_dir">
          service logs: {{ diagnostics.service_log_dir }}
        </div>
      </div>

      <!-- 最近一次上传的诊断包（zip 3 天后过期） -->
      <div v-if="lastUpload" class="mt-3 text-xs">
        <div class="flex flex-wrap items-center gap-2">
          <span class="font-medium">{{ t("settings.upload_diag") }}</span>
          <span class="text-surface-500">{{ lastUpload.at }}</span>
          <Button
            :label="t('settings.copy')"
            size="small"
            text
            icon="pi pi-copy"
            @click="copyLastUpload"
          />
        </div>
        <pre
          class="font-mono whitespace-pre-wrap break-all text-surface-600 dark:text-surface-300"
          >{{ lastUpload.urls }}</pre>
      </div>
    </template>
  </Card>
</template>

<style scoped lang="postcss"></style>
