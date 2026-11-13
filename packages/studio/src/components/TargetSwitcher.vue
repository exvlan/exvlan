<script setup lang="ts">
// 目标切换器：运行方式（本机内核怎么活）× 目标（界面连哪里）。
//
// 标题栏里显示当前「目标」；点开后可切运行方式、切目标、管理远程。
import { computed, nextTick, ref } from "vue";
import { useI18n } from "vue-i18n";
import { type as osType } from "@tauri-apps/plugin-os";
import { useToast } from "openvue";
import {
  LOCAL_TARGET,
  type RemoteTarget,
  type RuntimeKind,
  loadRemotes,
  remoteAddress,
  remoteMode,
  remoteUrl,
} from "~/composables/target";
import RemoteManager from "~/components/RemoteManager.vue";
import { gatewayPairStart, gatewaySystem, remotePing } from "~/composables/backend";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";

const props = defineProps<{
  runtime: RuntimeKind;
  targetId: string;
  disabled?: boolean;
}>();

const emit = defineEmits<{
  (e: "switch-runtime", runtime: RuntimeKind): void;
  (e: "switch-target", id: string): void;
  (e: "remotes-changed"): void;
}>();

const { t } = useI18n();
const toast = useToast();

// PrimeVue Menu 的实例类型在自动导入里是 any，这里显式标注避免 no-explicit-any 报错。
const menu = ref<{ toggle: (event: Event) => void } | null>(null);
const managerVisible = ref(false);
// 「对端出码」：本机就是被管端时，在这里生成一次性配对码给控制端。
const pairDialogVisible = ref(false);
const pairRole = ref<"readonly" | "config" | "admin">("readonly");
const pairBusy = ref(false);
const pairCode = ref("");
const pairExpires = ref(0);
const pairError = ref("");
const remotes = ref<RemoteTarget[]>(loadRemotes());
const testing = ref<string | null>(null);

// 移动端内核在进程内（FFI），没有可切换的运行方式；只保留目标。
const isMobile = computed(() => {
  const p = osType();
  return p === "android" || p === "ios";
});

/** `vServer · 10.254.1.1:2016`，没填名字时只显示地址。 */
function remoteLabel(remote: RemoteTarget): string {
  const addr = remoteAddress(remote);
  return remote.name ? `${remote.name} · ${addr}` : addr;
}

const targetText = computed(() => {
  if (props.targetId === LOCAL_TARGET) {
    return t("target.local");
  }
  const remote = remotes.value.find((r) => r.id === props.targetId);
  if (!remote) return t("target.missing");
  return remoteLabel(remote);
});

const runtimeText = computed(() => t(`runtime.${props.runtime}`));

function refresh() {
  remotes.value = loadRemotes();
}

async function toggle(event: Event) {
  // 必须先 refresh **再等一次 flush**：Menu 在自己的 toggle() 里同步读取
  // `model` props，若同一个 tick 里刚改完 remotes，子组件拿到的还是上一份
  // 列表（实测：刚添加的远程目标不出现在「目标」子菜单里，要再开一次才有）。
  refresh();
  await nextTick();
  menu.value?.toggle(event);
}

const runtimeItems = computed(() =>
  isMobile.value
    ? null
    : {
        label: t("runtime.title"),
        items: (["service", "normal", "off"] as RuntimeKind[]).map((kind) => ({
          label: t(`runtime.${kind}`),
          icon:
            props.runtime === kind
              ? "pi pi-check"
              : kind === "service"
                ? "pi pi-server"
                : kind === "normal"
                  ? "pi pi-desktop"
                  : "pi pi-power-off",
          command: () => {
            if (props.runtime !== kind) emit("switch-runtime", kind);
          },
        })),
      },
);

const items = computed(() => {
  const list: Record<string, unknown>[] = [];
  if (runtimeItems.value) {
    list.push(runtimeItems.value, { separator: true });
  }
  list.push({
    label: t("target.title"),
    items: [
      {
        label: t("target.local"),
        icon: props.targetId === LOCAL_TARGET ? "pi pi-check" : "pi pi-home",
        command: () => emit("switch-target", LOCAL_TARGET),
      },
      ...remotes.value.map((remote) => ({
        // 名字与地址都给出来：只显示 name 时看不出连的是哪台机器
        label: remoteLabel(remote),
        icon:
          props.targetId === remote.id
            ? "pi pi-check"
            : remoteMode(remote) === "gateway"
              ? "pi pi-shield"
              : "pi pi-globe",
        command: () => emit("switch-target", remote.id),
      })),
      { separator: true },
      {
        label: t("target.test_all"),
        icon: "pi pi-wave-pulse",
        command: () => void testAll(),
      },
      {
        label: t("target.manage"),
        icon: "pi pi-cog",
        command: () => {
          managerVisible.value = true;
        },
      },
      { separator: true },
      {
        label: t("target.generate_code"),
        icon: "pi pi-key",
        command: () => {
          pairCode.value = "";
          pairError.value = "";
          pairDialogVisible.value = true;
        },
      },
    ],
  });
  return list;
});

const roleOptions = computed(() => [
  { label: t("target.role_readonly"), value: "readonly" },
  { label: t("target.role_config"), value: "config" },
  { label: t("target.role_admin"), value: "admin" },
]);

async function generatePairCode() {
  pairBusy.value = true;
  pairError.value = "";
  pairCode.value = "";
  try {
    const value = await gatewayPairStart(pairRole.value);
    pairCode.value = String(value.code ?? "");
    pairExpires.value = Number(value.expires_in ?? 0);
    if (!pairCode.value) {
      pairError.value = JSON.stringify(value).slice(0, 200);
    }
  } catch (e) {
    pairError.value = String(e);
  } finally {
    pairBusy.value = false;
  }
}

async function copyPairCode() {
  if (!pairCode.value) return;
  try {
    await writeText(pairCode.value);
    toast.add({ severity: "success", summary: t("settings.copy"), life: 2000 });
  } catch (e) {
    toast.add({ severity: "error", summary: String(e), life: 6000 });
  }
}

async function testAll() {
  if (remotes.value.length === 0) {
    return;
  }
  const lines: string[] = [];
  for (const remote of remotes.value) {
    testing.value = remote.id;
    try {
      if (remoteMode(remote) === "gateway") {
        // 经 gateway：用 /system 探活（顺带验证 token 是否仍有效）。
        const system = await gatewaySystem(remote.id);
        lines.push(`${remote.name || remote.host} · gateway ${String(system.version ?? "ok")}`);
      } else {
        const ms = await remotePing(remoteUrl(remote));
        lines.push(`${remote.name || remote.host} · ${ms}ms`);
      }
    } catch (e: unknown) {
      lines.push(`${remote.name || remote.host} · ✗ ${String(e).slice(0, 80)}`);
    }
  }
  testing.value = null;
  toast.add({
    severity: "info",
    summary: t("target.test_all"),
    detail: lines.join("\n"),
    life: 8000,
  });
}

function onRemotesChanged() {
  refresh();
  emit("remotes-changed");
  if (props.targetId !== LOCAL_TARGET && !remotes.value.some((r) => r.id === props.targetId)) {
    emit("switch-target", LOCAL_TARGET);
  }
}
</script>

<template>
  <div class="flex items-center">
    <Button
      text
      size="small"
      :disabled="disabled"
      :loading="testing !== null"
      class="!px-2"
      @click="toggle"
    >
      <span class="flex items-center gap-1 text-sm">
        <i class="pi pi-globe text-[12px]" />
        <span>{{ targetText }}</span>
        <template v-if="!isMobile">
          <span class="text-surface-400">·</span>
          <span class="text-surface-500">{{ runtimeText }}</span>
        </template>
        <i class="pi pi-angle-down text-[10px]" />
      </span>
    </Button>
    <Menu ref="menu" :model="items" popup />
    <RemoteManager v-model:visible="managerVisible" @changed="onRemotesChanged" />

    <!-- 被管端出码：把 6 位码给对方，token 由本机在兑换时签发 -->
    <Dialog
      v-model:visible="pairDialogVisible"
      modal
      :header="t('target.pair_code_title')"
      :style="{ width: isMobile ? '100vw' : '420px' }"
    >
      <div class="flex flex-col gap-3">
        <div class="text-sm text-surface-500">{{ t("target.pair_code_desc") }}</div>
        <Select
          v-model="pairRole"
          :options="roleOptions"
          option-label="label"
          option-value="value"
        />
        <Button
          :label="t('target.generate')"
          icon="pi pi-key"
          :loading="pairBusy"
          @click="generatePairCode"
        />
        <div v-if="pairCode" class="flex flex-col items-center gap-1 py-2">
          <div class="font-mono text-3xl tracking-[0.3em]">{{ pairCode }}</div>
          <div class="text-xs text-surface-500">
            {{ t("target.pair_code_ttl", { seconds: pairExpires }) }}
          </div>
          <Button
            size="small"
            text
            icon="pi pi-copy"
            :label="t('settings.copy')"
            @click="copyPairCode"
          />
        </div>
        <div v-if="pairError" class="text-sm text-red-500">{{ pairError }}</div>
      </div>
    </Dialog>
  </div>
</template>
