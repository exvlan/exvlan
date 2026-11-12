<script setup lang="ts">
// 目标切换器：运行方式（本机内核怎么活）× 目标（界面连哪里）。
//
// 标题栏里显示当前「目标」；点开后可切运行方式、切目标、管理远程。
import { computed, nextTick, ref } from "vue";
import { useI18n } from "vue-i18n";
import { type as osType } from "@tauri-apps/plugin-os";
import { useToast } from "openvue";
import {
  DEFAULT_RPC_PORT,
  LOCAL_TARGET,
  type RemoteTarget,
  type RuntimeKind,
  loadRemotes,
  remoteUrl,
} from "~/composables/target";
import RemoteManager from "~/components/RemoteManager.vue";
import { remotePing } from "~/composables/backend";

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
const remotes = ref<RemoteTarget[]>(loadRemotes());
const testing = ref<string | null>(null);

// 移动端内核在进程内（FFI），没有可切换的运行方式；只保留目标。
const isMobile = computed(() => {
  const p = osType();
  return p === "android" || p === "ios";
});

/** `vServer · 10.254.1.1`，没填名字时只显示地址。 */
function remoteLabel(remote: RemoteTarget): string {
  const addr = `${remote.host}:${remote.port || DEFAULT_RPC_PORT}`;
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
        icon: props.targetId === remote.id ? "pi pi-check" : "pi pi-globe",
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
    ],
  });
  return list;
});

async function testAll() {
  if (remotes.value.length === 0) {
    return;
  }
  const lines: string[] = [];
  for (const remote of remotes.value) {
    testing.value = remote.id;
    try {
      const ms = await remotePing(remoteUrl(remote));
      lines.push(`${remote.name || remote.host} · ${ms}ms`);
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
  </div>
</template>
