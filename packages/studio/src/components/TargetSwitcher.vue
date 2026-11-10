<script setup lang="ts">
// 目标切换器：运行方式（本机内核怎么活）× 目标（界面连哪里）。
//
// 标题栏里显示当前「目标」；点开后可切运行方式、切目标、管理远程。
import { computed, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useToast } from "openvue";
import {
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
}>();

const { t } = useI18n();
const toast = useToast();

// PrimeVue Menu 的实例类型在自动导入里是 any，这里显式标注避免 no-explicit-any 报错。
const menu = ref<{ toggle: (event: Event) => void } | null>(null);
const managerVisible = ref(false);
const remotes = ref<RemoteTarget[]>(loadRemotes());
const testing = ref<string | null>(null);

const targetText = computed(() => {
  if (props.targetId === LOCAL_TARGET) {
    return t("target.local");
  }
  const remote = remotes.value.find((r) => r.id === props.targetId);
  if (!remote) return t("target.missing");
  return remote.name || `${remote.host}:${remote.port}`;
});

const runtimeText = computed(() => t(`runtime.${props.runtime}`));

function refresh() {
  remotes.value = loadRemotes();
}

function toggle(event: Event) {
  refresh();
  menu.value?.toggle(event);
}

const items = computed(() => [
  {
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
  { separator: true },
  {
    label: t("target.title"),
    items: [
      {
        label: t("target.local"),
        icon: props.targetId === LOCAL_TARGET ? "pi pi-check" : "pi pi-home",
        command: () => emit("switch-target", LOCAL_TARGET),
      },
      ...remotes.value.map((remote) => ({
        label: remote.name || `${remote.host}:${remote.port}`,
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
  },
]);

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
        <span class="text-surface-400">·</span>
        <span class="text-surface-500">{{ runtimeText }}</span>
        <i class="pi pi-angle-down text-[10px]" />
      </span>
    </Button>
    <Menu ref="menu" :model="items" popup />
    <RemoteManager v-model:visible="managerVisible" @changed="onRemotesChanged" />
  </div>
</template>
