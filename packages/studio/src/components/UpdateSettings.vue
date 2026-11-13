<script setup lang="ts">
// 设置 → 更新：通道选择 + 应用/内核两行状态 + 检查/安装 + 高级开关。
//
// 语义（见 docs/decisions-7.3.md §15）：
// - 单一通道，控制「当前通道及以下」所有版本（向下包含）；
// - 应用更新只在本机做；远端目标（经 gateway）只允许内核升级；
// - 不自动降级：只允许同 base 去掉 prerelease 后缀，跨 Mmpp 需手动安装旧包。
import { computed, onMounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { useToast } from "openvue";
import {
  type AppUpdateInfo,
  type KernelUpdateInfo,
  type UpdateChannel,
  type UpdatePrefs,
  appUpdateApply,
  appUpdateInfo,
  getUpdatePrefs,
  kernelInstallVersion,
  kernelUpdateInfo,
  setUpdateChannel,
  setUpdateFlag,
  skipUpdateVersion,
  unskipUpdateVersion,
} from "~/composables/backend";

const props = defineProps<{
  /** 当前目标是不是「经 gateway」的远端（应用更新只在本机做） */
  remote?: boolean;
}>();

const { t } = useI18n();
const toast = useToast();

const prefs = ref<UpdatePrefs | null>(null);
const appInfo = ref<AppUpdateInfo | null>(null);
const kernelInfo = ref<KernelUpdateInfo | null>(null);
const checking = ref(false);
const installing = ref<string | null>(null);
const chosenApp = ref("");
const chosenKernel = ref("");

const channels: UpdateChannel[] = ["stable", "rc", "beta", "dev", "nightly"];

const channelOptions = computed(() =>
  channels.map((value) => ({
    value,
    label: t(`update.channel_${value}`),
    desc: t(`update.channel_${value}_desc`),
  })),
);

const channelDesc = computed(() => {
  const current = prefs.value?.channel;
  return channelOptions.value.find((c) => c.value === current)?.desc ?? "";
});

const appChoices = computed(() => appInfo.value?.choices ?? []);
const kernelChoices = computed(() => kernelInfo.value?.choices ?? []);
const appTarget = computed(() => chosenApp.value || appInfo.value?.latest || "");
const kernelTarget = computed(() => chosenKernel.value || kernelInfo.value?.latest || "");

onMounted(async () => {
  try {
    prefs.value = await getUpdatePrefs();
  } catch (e) {
    console.warn("get_update_prefs failed", e);
  }
});

async function onChannel(value: UpdateChannel) {
  try {
    await setUpdateChannel(value);
    prefs.value = await getUpdatePrefs();
    await check();
  } catch (e) {
    toast.add({ severity: "error", summary: String(e), life: 8000 });
  }
}

async function onFlag(key: "auto_check" | "auto_install_kernel", value: boolean) {
  try {
    await setUpdateFlag(key, value);
    prefs.value = await getUpdatePrefs();
  } catch (e) {
    toast.add({ severity: "error", summary: String(e), life: 8000 });
  }
}

async function check() {
  checking.value = true;
  try {
    const [app, kernel] = await Promise.all([
      props.remote
        ? Promise.resolve(null)
        : appUpdateInfo().catch((e) => {
            console.warn("app_update_info failed", e);
            return null;
          }),
      kernelUpdateInfo().catch((e) => {
        console.warn("kernel_update_info failed", e);
        return null;
      }),
    ]);
    appInfo.value = app;
    kernelInfo.value = kernel;
    chosenApp.value = app?.latest ?? "";
    chosenKernel.value = kernel?.latest ?? "";
    if (!app?.available && !kernel?.available) {
      toast.add({ severity: "info", summary: t("update.up_to_date"), life: 3000 });
    }
  } finally {
    checking.value = false;
  }
}

async function installApp() {
  const version = appTarget.value;
  if (!version) return;
  installing.value = `app:${version}`;
  try {
    await appUpdateApply(version);
    toast.add({
      severity: "success",
      summary: t("update.install_started"),
      detail: version,
      life: 6000,
    });
  } catch (e) {
    toast.add({ severity: "error", summary: String(e), detail: version, life: 10000 });
  } finally {
    installing.value = null;
  }
}

async function installKernel() {
  const version = kernelTarget.value;
  if (!version) return;
  installing.value = `kernel:${version}`;
  try {
    await kernelInstallVersion(version);
    toast.add({
      severity: "success",
      summary: t("update.install_started"),
      detail: version,
      life: 6000,
    });
    kernelInfo.value = await kernelUpdateInfo().catch(() => kernelInfo.value);
  } catch (e) {
    toast.add({ severity: "error", summary: String(e), detail: version, life: 10000 });
  } finally {
    installing.value = null;
  }
}

async function onSkip() {
  const version = appInfo.value?.latest || kernelInfo.value?.latest;
  if (!version) return;
  try {
    await skipUpdateVersion(version);
    prefs.value = await getUpdatePrefs();
    toast.add({ severity: "info", summary: t("update.skipped"), detail: version, life: 4000 });
  } catch (e) {
    toast.add({ severity: "error", summary: String(e), life: 8000 });
  }
}

async function onUnskip() {
  try {
    await unskipUpdateVersion();
    prefs.value = await getUpdatePrefs();
    await check();
  } catch (e) {
    toast.add({ severity: "error", summary: String(e), life: 8000 });
  }
}
</script>

<template>
  <div class="flex flex-col gap-4">
    <div
      v-if="props.remote"
      class="rounded border border-amber-300 bg-amber-50 p-3 text-sm dark:border-amber-700 dark:bg-amber-950/40"
    >
      {{ t("update.remote_notice") }}
    </div>

    <div class="flex flex-col gap-2">
      <label class="font-medium">{{ t("update.channel") }}</label>
      <Select
        :model-value="prefs?.channel"
        :options="channelOptions"
        option-label="label"
        option-value="value"
        class="w-64"
        @update:model-value="(v: any) => onChannel(v as UpdateChannel)"
      />
      <div class="text-xs text-surface-500">{{ channelDesc }}</div>
      <div v-if="prefs?.env_override" class="text-xs text-amber-600">
        {{ t("update.env_override", { value: prefs.env_override }) }}
      </div>
    </div>

    <Divider />

    <!-- 应用 -->
    <div class="flex flex-col gap-1">
      <div class="flex items-center gap-3">
        <span class="w-16 font-medium">{{ t("update.app") }}</span>
        <span class="font-mono text-sm">
          {{ appInfo?.current || "—" }}
          <template v-if="appInfo?.latest && appInfo.latest !== appInfo.current">
            → {{ appInfo.latest }}
          </template>
        </span>
        <Tag v-if="appInfo?.available" severity="success" :value="t('update.available')" />
        <Tag v-else-if="appInfo" severity="secondary" :value="t('update.up_to_date_short')" />
      </div>
      <div v-if="!props.remote && appChoices.length > 1" class="flex items-center gap-2 text-xs">
        <span class="text-surface-500">{{ t("update.choose_version") }}</span>
        <Select
          v-model="chosenApp"
          :options="appChoices"
          option-label="version"
          option-value="version"
          class="w-56"
        >
          <template #option="slotProps">
            <div class="flex flex-col">
              <span>{{ slotProps.option.version }}</span>
              <span v-if="slotProps.option.reason" class="text-xs text-red-500">
                {{ slotProps.option.reason }}
              </span>
            </div>
          </template>
        </Select>
      </div>
      <div v-if="appInfo?.notes" class="whitespace-pre-line text-xs text-surface-500">
        {{ appInfo.notes }}
      </div>
      <div class="flex gap-2">
        <Button
          size="small"
          :label="t('update.install')"
          icon="pi pi-download"
          :disabled="props.remote || !appInfo?.available"
          :loading="installing === `app:${appTarget}`"
          @click="installApp"
        />
        <Button
          v-if="appInfo?.available"
          size="small"
          text
          :label="t('update.skip')"
          @click="onSkip"
        />
      </div>
      <div v-if="prefs?.skipped" class="flex items-center gap-2 text-xs text-surface-500">
        {{ t("update.skipped_hint", { version: prefs.skipped }) }}
        <Button size="small" text :label="t('update.unskip')" @click="onUnskip" />
      </div>
    </div>

    <Divider />

    <!-- 内核 -->
    <div class="flex flex-col gap-1">
      <div class="flex items-center gap-3">
        <span class="w-16 font-medium">{{ t("update.kernel") }}</span>
        <span class="font-mono text-sm">
          {{ kernelInfo?.current || "—" }}
          <template v-if="kernelInfo?.latest && kernelInfo.latest !== kernelInfo.current">
            → {{ kernelInfo.latest }}
          </template>
        </span>
        <Tag v-if="kernelInfo?.available" severity="success" :value="t('update.available')" />
        <Tag v-else-if="kernelInfo" severity="secondary" :value="t('update.up_to_date_short')" />
      </div>
      <div v-if="kernelChoices.length > 1" class="flex items-center gap-2 text-xs">
        <span class="text-surface-500">{{ t("update.choose_version") }}</span>
        <Select
          v-model="chosenKernel"
          :options="kernelChoices"
          option-label="version"
          option-value="version"
          class="w-56"
        >
          <template #option="slotProps">
            <div class="flex flex-col">
              <span :class="slotProps.option.compatible === false ? 'opacity-50' : ''">
                {{ slotProps.option.version }}
              </span>
              <span v-if="slotProps.option.reason" class="text-xs text-red-500">
                {{ slotProps.option.reason }}
              </span>
            </div>
          </template>
        </Select>
      </div>
      <div class="flex gap-2">
        <Button
          size="small"
          :label="t('update.install')"
          icon="pi pi-download"
          :disabled="!kernelTarget || kernelInfo?.available === false"
          :loading="installing === `kernel:${kernelTarget}`"
          @click="installKernel"
        />
      </div>
      <div v-if="props.remote" class="text-xs text-surface-500">
        {{ t("update.remote_kernel_hint") }}
      </div>
    </div>

    <Divider />

    <div class="flex flex-col gap-2">
      <div class="flex items-center gap-2">
        <ToggleSwitch
          :model-value="prefs?.auto_check ?? true"
          @update:model-value="(v: boolean) => onFlag('auto_check', v)"
        />
        <span class="text-sm">{{ t("update.auto_check") }}</span>
      </div>
      <div class="flex items-center gap-2">
        <ToggleSwitch
          :model-value="prefs?.auto_install_kernel ?? false"
          @update:model-value="(v: boolean) => onFlag('auto_install_kernel', v)"
        />
        <span class="text-sm">{{ t("update.auto_install_kernel") }}</span>
      </div>
      <div class="text-xs text-surface-500">{{ t("update.auto_install_kernel_hint") }}</div>
    </div>

    <div class="flex items-center gap-2">
      <Button
        size="small"
        :label="t('update.check')"
        icon="pi pi-refresh"
        :loading="checking"
        @click="check"
      />
      <span class="text-xs text-surface-500">{{ t("update.teaser") }}</span>
    </div>
  </div>
</template>
