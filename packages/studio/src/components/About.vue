<script setup lang="ts">
import pkg from '~/../package.json'
import { getExvlanVersion } from '~/composables/backend'
import {
  appDownloadUpdate,
  appInstallUpdate,
  appUpdateInfo,
  type AppUpdateInfo,
} from '~/composables/backend'
import { useToast } from 'openvue'

const { t } = useI18n()
const toast = useToast()

const appVersion = ref(pkg.version)
const kernelVersion = ref('')
const appInfo = ref<AppUpdateInfo | null>(null)
const checking = ref(false)
const downloading = ref(false)

onMounted(async () => {
  kernelVersion.value = await getExvlanVersion().catch(() => '')
})

async function onCheckApp() {
  checking.value = true
  try {
    appInfo.value = await appUpdateInfo()
    if (!appInfo.value.available) {
      toast.add({ severity: 'info', summary: t('upgrade.app_up_to_date'), life: 3000 })
    }
  }
  catch (e: any) {
    toast.add({ severity: 'error', summary: t('error'), detail: String(e), life: 8000 })
  }
  finally {
    checking.value = false
  }
}

async function onDownloadApp() {
  const version = appInfo.value?.latest
  if (!version) {
    return
  }
  downloading.value = true
  try {
    const path = await appDownloadUpdate(version)
    await appInstallUpdate(path)
    toast.add({
      severity: 'success',
      summary: t('upgrade.downloaded'),
      detail: path,
      life: 8000,
    })
  }
  catch (e: any) {
    toast.add({ severity: 'error', summary: t('error'), detail: String(e), life: 10000 })
  }
  finally {
    downloading.value = false
  }
}
</script>

<template>
  <Card>
    <template #title>
      Exvlan
    </template>
    <template #content>
      <p class="mb-3">
        {{ t('about.description') }}
      </p>

      <!-- 应用版本 -->
      <div class="flex flex-wrap items-center gap-3 mb-2">
        <span class="font-medium">{{ t('upgrade.app_version') }}:</span>
        <span>{{ appVersion }}</span>
        <Button
          :label="t('upgrade.check')" size="small" severity="secondary"
          :loading="checking" @click="onCheckApp"
        />
        <template v-if="appInfo">
          <span v-if="appInfo.available" class="text-sm text-primary">
            {{ t('upgrade.new_version', { version: appInfo.latest }) }}
          </span>
          <span v-else class="text-sm text-green-600">{{ t('upgrade.app_up_to_date') }}</span>
        </template>
      </div>
      <div v-if="appInfo?.available" class="flex flex-wrap items-center gap-3 mb-3">
        <Button :label="t('upgrade.download_and_install')" size="small" :loading="downloading" @click="onDownloadApp" />
        <span v-if="appInfo.notes" class="text-sm text-gray-500 whitespace-pre-wrap">{{ appInfo.notes }}</span>
      </div>

      <!-- 内核版本 -->
      <div class="flex flex-wrap items-center gap-3">
        <span class="font-medium">{{ t('upgrade.kernel_version') }}:</span>
        <span>{{ kernelVersion || t('upgrade.unknown') }}</span>
        <span class="text-sm text-gray-500">{{ t('upgrade.kernel_hint') }}</span>
      </div>
    </template>
  </Card>
</template>

<style scoped lang="postcss">
</style>
