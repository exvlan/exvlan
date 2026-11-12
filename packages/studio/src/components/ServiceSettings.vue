<script setup lang="ts">
// 「设置 → 服务」：特权服务的访问范围与维护操作。
//
// 设计要点：
//   · RPC 白名单由服务扫描 `<config-dir>/*.toml` 的虚拟网段自动推导，
//     这里只做「只读展示 + 额外追加」，避免用户把内网/公网整段放开
//   · 网段改了要重启内核才生效 —— 用 needs_restart 明确提示，不自动重启
//   · 卸载是不可逆操作，需要勾选确认
import { computed, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { useToast } from "openvue";
import {
  type PortOwnerInfo,
  type WhitelistInfo,
  portCleanup,
  portOwnerInfo,
  serviceExtraWhitelistSet,
  serviceReinstall,
  serviceRestart,
  serviceUninstall,
  serviceWhitelistInfo,
} from "~/composables/backend";

const visible = defineModel<boolean>("visible", { default: false });
const emit = defineEmits<{
  (e: "changed"): void;
  /** 卸载完成：外层需要把运行方式回落为普通模式 */
  (e: "uninstalled"): void;
}>();

const { t } = useI18n();
const toast = useToast();

const info = ref<WhitelistInfo | null>(null);
const loading = ref(false);
const saving = ref(false);
const restarting = ref(false);
const uninstalling = ref(false);
const owner = ref<PortOwnerInfo | null>(null);
const extraDraft = ref("");
const confirmUninstall = ref(false);

const effectiveList = computed(() =>
  (info.value?.effective ?? "")
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean),
);

async function refresh() {
  loading.value = true;
  try {
    info.value = await serviceWhitelistInfo();
    owner.value = await portOwnerInfo().catch(() => null);
  } catch (e: unknown) {
    toast.add({ severity: "error", summary: t("error"), detail: String(e), life: 8000 });
  } finally {
    loading.value = false;
  }
}

watch(visible, (open) => {
  if (open) {
    confirmUninstall.value = false;
    extraDraft.value = "";
    void refresh();
  }
});

async function addExtra() {
  const value = extraDraft.value.trim();
  if (!value) return;
  const next = [...(info.value?.extra ?? []), value];
  await saveExtra(next);
}

async function removeExtra(item: string) {
  await saveExtra((info.value?.extra ?? []).filter((entry) => entry !== item));
}

async function saveExtra(list: string[]) {
  saving.value = true;
  try {
    info.value = await serviceExtraWhitelistSet(list);
    extraDraft.value = "";
    toast.add({
      severity: "success",
      summary: t("settings.privileged_service"),
      detail: t("service_settings.extra_saved"),
      life: 4000,
    });
    emit("changed");
  } catch (e: unknown) {
    toast.add({ severity: "error", summary: t("error"), detail: String(e), life: 12000 });
  } finally {
    saving.value = false;
  }
}

async function onRestart() {
  restarting.value = true;
  try {
    await serviceRestart();
    await refresh();
    emit("changed");
    toast.add({
      severity: "success",
      summary: t("settings.privileged_service"),
      detail: t("service_settings.restarted"),
      life: 4000,
    });
  } catch (e: unknown) {
    toast.add({ severity: "error", summary: t("error"), detail: String(e), life: 12000 });
  } finally {
    restarting.value = false;
  }
}

async function onReinstall() {
  restarting.value = true;
  try {
    await serviceReinstall({
      config_dir: "",
      rpc_portal: "127.0.0.1:1995",
      file_log_level: "info",
      file_log_dir: "",
      config_server: undefined,
    });
    await refresh();
    emit("changed");
  } catch (e: unknown) {
    toast.add({ severity: "error", summary: t("error"), detail: String(e), life: 12000 });
  } finally {
    restarting.value = false;
  }
}

async function onUninstall() {
  if (!confirmUninstall.value) return;
  uninstalling.value = true;
  try {
    const next = await serviceUninstall("normal");
    toast.add({
      severity: "warn",
      summary: t("service_settings.uninstall_done"),
      detail: t("service_settings.uninstall_done_detail", { mode: next }),
      life: 10000,
    });
    emit("uninstalled");
    visible.value = false;
  } catch (e: unknown) {
    toast.add({ severity: "error", summary: t("error"), detail: String(e), life: 12000 });
  } finally {
    uninstalling.value = false;
  }
}

async function onCleanupPort() {
  try {
    await portCleanup();
    toast.add({
      severity: "success",
      summary: t("service_settings.port_free"),
      life: 4000,
    });
    await refresh();
  } catch (e: unknown) {
    toast.add({ severity: "error", summary: t("error"), detail: String(e), life: 12000 });
  }
}
</script>

<template>
  <Dialog
    v-model:visible="visible"
    modal
    :header="t('settings.privileged_service')"
    :style="{ width: '52vw', minWidth: '460px' }"
  >
    <div class="flex flex-col gap-4">
      <!-- 访问范围 -->
      <div class="flex flex-col gap-2">
        <div class="flex items-center justify-between">
          <div class="font-medium">{{ t("service_settings.rpc_scope") }}</div>
          <Button size="small" text icon="pi pi-refresh" :loading="loading" @click="refresh" />
        </div>

        <div class="rounded border border-surface-300 p-3 text-sm dark:border-surface-600">
          <div v-for="item in effectiveList" :key="item" class="font-mono">{{ item }}</div>
          <div class="mt-2 text-surface-500">{{ t("service_settings.rpc_scope_hint") }}</div>
        </div>

        <Message v-if="info?.needs_restart" severity="warn" :closable="false">
          <span class="flex flex-col gap-2">
            <span>{{ t("service_settings.needs_restart") }}</span>
            <Button
              size="small"
              :label="t('service_settings.apply_and_restart')"
              :loading="restarting"
              @click="onRestart"
            />
          </span>
        </Message>

        <div v-if="info?.notes?.length" class="text-xs text-surface-500">
          <div v-for="note in info.notes" :key="note">· {{ note }}</div>
        </div>
      </div>

      <!-- 额外网段 -->
      <div class="flex flex-col gap-2">
        <div class="font-medium">{{ t("service_settings.extra") }}</div>
        <div class="text-xs text-surface-500">
          {{ t("service_settings.extra_hint", { max: info?.max_extra ?? 8 }) }}
        </div>
        <div class="flex items-center gap-2">
          <InputText
            v-model="extraDraft"
            class="flex-1"
            placeholder="192.168.50.0/24"
            @keyup.enter="addExtra"
          />
          <Button size="small" :label="t('web.common.add')" :loading="saving" @click="addExtra" />
        </div>
        <div
          v-for="item in info?.extra ?? []"
          :key="item"
          class="flex items-center justify-between rounded bg-surface-100 px-2 py-1 text-sm dark:bg-surface-800"
        >
          <span class="font-mono">{{ item }}</span>
          <Button
            size="small"
            text
            severity="danger"
            icon="pi pi-times"
            @click="removeExtra(item)"
          />
        </div>
      </div>

      <!-- 网络 MTU -->
      <div class="flex flex-col gap-1">
        <div class="font-medium">
          {{ t("service_settings.mtu") }}
          <span class="ml-1 font-mono text-sm text-surface-500">{{ info?.tun_mtu ?? "-" }}</span>
        </div>
        <div class="text-xs text-surface-500">{{ t("service_settings.mtu_hint") }}</div>
      </div>

      <!-- 端口占用 -->
      <div class="flex flex-col gap-2">
        <div class="font-medium">{{ t("service_settings.port") }}</div>
        <div class="text-sm">
          <template v-if="owner?.free">{{ t("service_settings.port_free") }}</template>
          <template v-else-if="owner?.pid">
            pid={{ owner.pid }} · {{ owner.name }}
            <span v-if="!owner.is_our_kernel" class="text-amber-600">
              （{{ t("service_settings.port_foreign") }}）
            </span>
            <div class="break-all font-mono text-xs text-surface-500">{{ owner.path }}</div>
            <Button
              v-if="owner.is_our_kernel"
              size="small"
              severity="danger"
              class="mt-1"
              :label="t('service_settings.port_kill')"
              @click="onCleanupPort"
            />
          </template>
          <template v-else>-</template>
        </div>
      </div>

      <!-- 维护 -->
      <div
        class="flex flex-wrap items-center gap-2 border-t border-surface-200 pt-3 dark:border-surface-700"
      >
        <Button
          size="small"
          severity="secondary"
          :label="t('settings.service_reinstall')"
          :loading="restarting"
          @click="onReinstall"
        />
        <Button
          size="small"
          severity="secondary"
          :label="t('settings.service_restart')"
          :loading="restarting"
          @click="onRestart"
        />
      </div>

      <div class="flex flex-col gap-2 border-t border-surface-200 pt-3 dark:border-surface-700">
        <div class="text-sm text-surface-500">{{ t("service_settings.uninstall_hint") }}</div>
        <label class="flex items-center gap-2 text-sm">
          <input v-model="confirmUninstall" type="checkbox" class="h-4 w-4" />
          <span>{{ t("service_settings.uninstall_confirm") }}</span>
        </label>
        <Button
          size="small"
          severity="danger"
          class="self-start"
          :label="t('service_settings.uninstall')"
          :disabled="!confirmUninstall"
          :loading="uninstalling"
          @click="onUninstall"
        />
      </div>
    </div>

    <template #footer>
      <Button :label="t('web.common.close')" @click="visible = false" />
    </template>
  </Dialog>
</template>
