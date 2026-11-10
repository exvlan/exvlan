<script setup lang="ts">
// 远程管理：目标列表（主机 + 端口，无密钥、无 HTTPS——只走 mesh 内网）。
import { computed, ref, watch } from "vue";
import { type as osType } from "@tauri-apps/plugin-os";
import { useI18n } from "vue-i18n";
import { useToast } from "openvue";
import {
  DEFAULT_RPC_PORT,
  type RemoteTarget,
  loadRemotes,
  newRemote,
  parseAddress,
  remoteUrl,
  saveRemotes,
} from "~/composables/target";
import { remotePing } from "~/composables/backend";

const visible = defineModel<boolean>("visible", { default: false });
const emit = defineEmits<{ (e: "changed"): void }>();

const { t } = useI18n();

// Android 上没有「窗口」，用全屏对话框（与其它移动端页面一致）。
const isMobile = computed(() => {
  const p = osType();
  return p === "android" || p === "ios";
});
const dialogStyle = computed(() =>
  isMobile.value ? { width: "100vw", height: "100vh" } : { width: "46vw", minWidth: "420px" },
);
const toast = useToast();

const rows = ref<RemoteTarget[]>([]);
const editing = ref<RemoteTarget | null>(null);
const address = ref("");
const busy = ref(false);
const testResult = ref<Record<string, string>>({});

watch(visible, (open) => {
  if (open) {
    rows.value = loadRemotes().map((r) => ({ ...r }));
    editing.value = null;
    address.value = "";
    testResult.value = {};
  }
});

function startAdd() {
  editing.value = newRemote();
  address.value = "";
}

function startEdit(row: RemoteTarget) {
  editing.value = { ...row };
  address.value = `${row.host}:${row.port || DEFAULT_RPC_PORT}`;
}

function cancelEdit() {
  editing.value = null;
  address.value = "";
}

async function commitEdit() {
  if (!editing.value) return;
  const parsed = parseAddress(address.value);
  if (!parsed) {
    toast.add({ severity: "error", summary: t("target.invalid_address"), life: 6000 });
    return;
  }
  const next = { ...editing.value, host: parsed.host, port: parsed.port };
  const index = rows.value.findIndex((r) => r.id === next.id);
  if (index >= 0) {
    rows.value[index] = next;
  } else {
    rows.value.push(next);
  }
  await persist();
  editing.value = null;
  address.value = "";
}

async function remove(row: RemoteTarget) {
  rows.value = rows.value.filter((r) => r.id !== row.id);
  await persist();
}

async function persist() {
  busy.value = true;
  try {
    await saveRemotes(rows.value);
    emit("changed");
  } finally {
    busy.value = false;
  }
}

async function test(row: RemoteTarget) {
  testResult.value = { ...testResult.value, [row.id]: "…" };
  try {
    const ms = await remotePing(remoteUrl(row));
    testResult.value = { ...testResult.value, [row.id]: `${ms}ms` };
  } catch (e: unknown) {
    testResult.value = { ...testResult.value, [row.id]: `✗ ${String(e).slice(0, 80)}` };
  }
}
</script>

<template>
  <Dialog v-model:visible="visible" modal :header="t('target.manage')" :style="dialogStyle">
    <div class="flex flex-col gap-3">
      <p class="text-sm text-surface-500">{{ t("target.hint") }}</p>

      <div v-for="row in rows" :key="row.id" class="flex items-center gap-2">
        <div class="flex-1">
          <div class="font-medium">{{ row.name || `${row.host}:${row.port}` }}</div>
          <div class="text-xs text-surface-500">
            {{ row.host }}:{{ row.port || DEFAULT_RPC_PORT }}
            <span v-if="testResult[row.id]" class="ml-2">{{ testResult[row.id] }}</span>
          </div>
        </div>
        <Button
          size="small"
          text
          icon="pi pi-wave-pulse"
          :aria-label="t('target.test')"
          @click="test(row)"
        />
        <Button
          size="small"
          text
          icon="pi pi-pencil"
          :aria-label="t('settings.edit')"
          @click="startEdit(row)"
        />
        <Button
          size="small"
          text
          severity="danger"
          icon="pi pi-trash"
          :aria-label="t('settings.delete')"
          @click="remove(row)"
        />
      </div>

      <div
        v-if="editing"
        class="flex flex-col gap-2 rounded border border-surface-300 p-3 dark:border-surface-600"
      >
        <label class="text-sm">{{ t("target.name") }}</label>
        <InputText v-model="editing.name" :placeholder="t('target.name_placeholder')" />
        <label class="text-sm">{{ t("target.address") }}</label>
        <InputText
          v-model="address"
          :placeholder="`10.254.0.7:${DEFAULT_RPC_PORT}`"
          @keyup.enter="commitEdit"
        />
        <div class="flex justify-end gap-2">
          <Button size="small" text :label="t('web.common.cancel')" @click="cancelEdit" />
          <Button size="small" :label="t('web.common.save')" :loading="busy" @click="commitEdit" />
        </div>
      </div>
    </div>

    <template #footer>
      <Button
        :label="t('target.add')"
        icon="pi pi-plus"
        text
        :disabled="!!editing"
        @click="startAdd"
      />
      <Button :label="t('web.common.close')" @click="visible = false" />
    </template>
  </Dialog>
</template>
