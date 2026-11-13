<script setup lang="ts">
// 远程管理：目标列表。
//
// 两种连接方式（由用户显式选择，见 docs/decisions-7.3.md §14.10）：
// - 直连内核（1995）：无鉴权，靠对端 RPC 白名单；
// - 经 gateway（默认 2016）：Bearer token，token 由**对端**在配对时签发，
//   本机只保存（Rust 侧 `remote-tokens.json`，0600），本版本对其只读。
import { computed, ref, watch } from "vue";
import { type as osType } from "@tauri-apps/plugin-os";
import { useI18n } from "vue-i18n";
import { useToast } from "openvue";
import {
  DEFAULT_GATEWAY_PORT,
  DEFAULT_RPC_PORT,
  type RemoteMode,
  type RemoteTarget,
  loadRemotes,
  newRemote,
  parseRemoteHost,
  remoteAddress,
  remoteMode,
  remoteUrl,
  saveRemotes,
} from "~/composables/target";
import {
  type GatewayRemoteSummary,
  gatewayForget,
  gatewayPair,
  gatewayRemotes,
  gatewaySystem,
  remotePing,
} from "~/composables/backend";

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
const mode = ref<RemoteMode>("direct");
const address = ref("");
const port = ref<number>(DEFAULT_RPC_PORT);
const busy = ref(false);
const testResult = ref<Record<string, string>>({});
/** 已配对目标（Rust 侧持有 token，这里只有摘要）。 */
const paired = ref<Record<string, GatewayRemoteSummary>>({});
/** 正在输入配对码的目标 id。 */
const pairingId = ref<string | null>(null);
const pairCode = ref("");

watch(visible, (open) => {
  if (open) {
    rows.value = loadRemotes().map((r) => ({ ...r }));
    editing.value = null;
    address.value = "";
    testResult.value = {};
    pairingId.value = null;
    pairCode.value = "";
    void refreshPaired();
  }
});

async function refreshPaired() {
  try {
    const list = await gatewayRemotes();
    paired.value = Object.fromEntries(list.map((item) => [item.id, item]));
  } catch (e) {
    // 读不到配对信息不影响直连目标的使用，但要留痕（不静默）。
    console.warn("gateway_remotes failed", e);
  }
}

function startAdd() {
  editing.value = newRemote();
  mode.value = "direct";
  address.value = "";
  port.value = DEFAULT_RPC_PORT;
}

function startEdit(row: RemoteTarget) {
  editing.value = { ...row };
  mode.value = remoteMode(row);
  address.value = row.host;
  port.value = row.port || (mode.value === "gateway" ? DEFAULT_GATEWAY_PORT : DEFAULT_RPC_PORT);
}

function cancelEdit() {
  editing.value = null;
  address.value = "";
}

/** 切换连接方式时同步默认端口（用户仍可手改）。 */
watch(mode, (next) => {
  port.value = next === "gateway" ? DEFAULT_GATEWAY_PORT : DEFAULT_RPC_PORT;
});

async function commitEdit() {
  if (!editing.value) return;
  const parsed = parseRemoteHost(address.value, mode.value);
  if (!parsed) {
    toast.add({ severity: "error", summary: t("target.invalid_address"), life: 6000 });
    return;
  }
  if ("error" in parsed) {
    toast.add({
      severity: "error",
      summary: t("target.port_fixed", { port: DEFAULT_RPC_PORT }),
      life: 8000,
    });
    return;
  }
  // gateway：地址里带端口时以它为准，否则用「端口」输入框（默认 2016）。
  const nextPort =
    mode.value === "gateway"
      ? (parsed.port ?? port.value ?? DEFAULT_GATEWAY_PORT)
      : DEFAULT_RPC_PORT;
  const next: RemoteTarget = {
    ...editing.value,
    host: parsed.host,
    mode: mode.value,
    port: nextPort,
  };
  const index = rows.value.findIndex((r) => r.id === next.id);
  if (index >= 0) {
    rows.value[index] = next;
  } else {
    rows.value.push(next);
  }
  await persist();
  editing.value = null;
  address.value = "";
  // 新建的 gateway 目标直接进入配对流程（否则切过去一定连不上）。
  if (mode.value === "gateway" && !paired.value[next.id]) {
    startPair(next);
  }
}

async function remove(row: RemoteTarget) {
  rows.value = rows.value.filter((r) => r.id !== row.id);
  if (paired.value[row.id]) {
    try {
      await gatewayForget(row.id);
    } catch (e) {
      // 本地记录可能已经消失，删目标本身不该被它挡住。
      console.warn("gateway_forget failed", row.id, e);
    }
  }
  await persist();
  await refreshPaired();
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
  if (remoteMode(row) === "gateway") {
    try {
      const system = await gatewaySystem(row.id);
      const version = String(system.version ?? system.service ?? "ok");
      testResult.value = { ...testResult.value, [row.id]: `gateway ${version}`.slice(0, 40) };
    } catch (e) {
      testResult.value = { ...testResult.value, [row.id]: `✗ ${String(e).slice(0, 80)}` };
    }
    return;
  }
  try {
    const ms = await remotePing(remoteUrl(row));
    testResult.value = { ...testResult.value, [row.id]: `${ms}ms` };
  } catch (e) {
    testResult.value = { ...testResult.value, [row.id]: `✗ ${String(e).slice(0, 80)}` };
  }
}

function startPair(row: RemoteTarget) {
  pairingId.value = row.id;
  pairCode.value = "";
}

function cancelPair() {
  pairingId.value = null;
  pairCode.value = "";
}

async function submitPair(row: RemoteTarget) {
  const code = pairCode.value.trim();
  if (!/^\d{6}$/.test(code)) {
    toast.add({ severity: "error", summary: t("target.pair_code_invalid"), life: 6000 });
    return;
  }
  busy.value = true;
  try {
    const info = await gatewayPair({
      targetId: row.id,
      host: row.host,
      port: row.port,
      code,
      client: `exvlan-studio-${osType()}`,
    });
    toast.add({
      severity: "success",
      summary: t("target.pair_done"),
      detail: info.scopes?.length ? info.scopes.join(", ") : undefined,
      life: 6000,
    });
    cancelPair();
    await refreshPaired();
  } catch (e) {
    toast.add({
      severity: "error",
      summary: t("target.pair_failed"),
      detail: String(e).slice(0, 300),
      life: 10000,
    });
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <Dialog v-model:visible="visible" modal :header="t('target.manage')" :style="dialogStyle">
    <div class="flex flex-col gap-3">
      <p class="text-sm text-surface-500">{{ t("target.hint") }}</p>

      <div v-for="row in rows" :key="row.id" class="flex flex-col gap-1">
        <div class="flex items-center gap-2">
          <div class="flex-1">
            <div class="flex items-center gap-2 font-medium">
              <span>{{ row.name || remoteAddress(row) }}</span>
              <Tag
                :value="
                  remoteMode(row) === 'gateway'
                    ? t('target.mode_gateway_short')
                    : t('target.mode_direct_short')
                "
                :severity="remoteMode(row) === 'gateway' ? 'success' : 'secondary'"
              />
              <Tag
                v-if="remoteMode(row) === 'gateway'"
                :value="paired[row.id] ? t('target.paired') : t('target.not_paired')"
                :severity="paired[row.id] ? 'info' : 'warn'"
              />
            </div>
            <div class="text-xs text-surface-500">
              {{ remoteAddress(row) }}
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
            v-if="remoteMode(row) === 'gateway'"
            size="small"
            text
            icon="pi pi-key"
            :aria-label="t('target.pair')"
            :tooltip="t('target.pair')"
            @click="startPair(row)"
          />
          <Button
            size="small"
            text
            icon="pi pi-pencil"
            :aria-label="t('web.common.edit')"
            @click="startEdit(row)"
          />
          <Button
            size="small"
            text
            severity="danger"
            icon="pi pi-trash"
            :aria-label="t('web.common.delete')"
            @click="remove(row)"
          />
        </div>

        <!-- 配对：输入对端生成的 6 位码（对端签发 token） -->
        <div
          v-if="pairingId === row.id"
          class="flex flex-col gap-2 rounded border border-surface-300 p-3 dark:border-surface-600"
        >
          <div class="text-sm">{{ t("target.pair_title") }}</div>
          <div class="text-xs text-surface-500">{{ t("target.pair_hint") }}</div>
          <InputText
            v-model="pairCode"
            :placeholder="t('target.pair_code')"
            maxlength="6"
            class="w-40 font-mono text-lg tracking-widest"
            @keyup.enter="submitPair(row)"
          />
          <div class="flex justify-end gap-2">
            <Button size="small" text :label="t('web.common.cancel')" @click="cancelPair" />
            <Button
              size="small"
              :label="t('target.pair')"
              :loading="busy"
              @click="submitPair(row)"
            />
          </div>
        </div>
      </div>

      <div
        v-if="editing"
        class="flex flex-col gap-2 rounded border border-surface-300 p-3 dark:border-surface-600"
      >
        <label class="text-sm">{{ t("target.name") }}</label>
        <InputText v-model="editing.name" :placeholder="t('target.name_placeholder')" />

        <label class="text-sm">{{ t("target.mode") }}</label>
        <Select
          v-model="mode"
          :options="[
            { label: t('target.mode_gateway'), value: 'gateway' },
            { label: t('target.mode_direct'), value: 'direct' },
          ]"
          option-label="label"
          option-value="value"
        />
        <div class="text-xs text-surface-500">{{ t("target.mode_hint") }}</div>

        <label class="text-sm">{{ t("target.address") }}</label>
        <InputText v-model="address" :placeholder="`10.254.1.1`" @keyup.enter="commitEdit" />

        <template v-if="mode === 'gateway'">
          <label class="text-sm">{{ t("target.port_gateway") }}</label>
          <InputNumber v-model="port" :min="1" :max="65535" :use-grouping="false" />
          <div class="text-xs text-surface-500">
            {{ t("target.port_gateway_hint", { port: DEFAULT_GATEWAY_PORT }) }}
          </div>
        </template>
        <div v-else class="text-xs text-surface-500">
          {{ t("target.port_hint", { port: DEFAULT_RPC_PORT }) }}
        </div>

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
