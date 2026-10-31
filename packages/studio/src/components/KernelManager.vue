<script setup lang="ts">
import { ref } from 'vue'
import { useToast } from 'openvue'
import { getExvlanVersion, kernelInstallVersion, kernelUpdateInfo } from '~/composables/backend'

interface UpdateInfo {
  platform: string
  current?: string
  latest?: string
  available: boolean
  versions: string[]
  source: string
}

const { t } = useI18n()
const toast = useToast()

const currentVersion = ref('')
const source = ref('')
const info = ref<UpdateInfo | null>(null)
const checking = ref(false)
const installing = ref<string | null>(null)

async function refreshCurrentVersion() {
  try {
    currentVersion.value = await getExvlanVersion()
  }
  catch {
    currentVersion.value = ''
  }
}
refreshCurrentVersion()

async function onCheck() {
  checking.value = true
  try {
    info.value = await kernelUpdateInfo(source.value || undefined)
    if (info.value?.source && !source.value)
      source.value = info.value.source
    if (!info.value?.available) {
      toast.add({ severity: 'info', summary: t('kernel.up_to_date'), life: 3000 })
    }
  }
  catch (e: any) {
    toast.add({ severity: 'error', summary: t('error'), detail: String(e), life: 10000 })
  }
  finally {
    checking.value = false
  }
}

async function onInstall(version: string) {
  installing.value = version
  try {
    await kernelInstallVersion(version, source.value || undefined)
    toast.add({
      severity: 'success',
      summary: t('kernel.install_success'),
      detail: version,
      life: 5000,
    })
    await refreshCurrentVersion()
    await onCheck()
  }
  catch (e: any) {
    toast.add({ severity: 'error', summary: t('error'), detail: String(e), life: 10000 })
  }
  finally {
    installing.value = null
  }
}
</script>

<template>
  <div class="flex flex-col gap-3 border-t pt-3">
    <div class="flex items-center justify-between">
      <span class="font-medium">{{ t('kernel.title') }}</span>
      <span class="text-sm text-gray-500">
        {{ t('kernel.current') }}: {{ currentVersion || t('kernel.unknown') }}
        <template v-if="info?.platform"> · {{ info.platform }}</template>
      </span>
    </div>

    <div class="flex flex-col gap-1">
      <label for="kernel-source" class="text-sm">{{ t('kernel.source') }}</label>
      <InputText id="kernel-source" v-model="source" :placeholder="t('kernel.source_placeholder')" />
    </div>

    <div class="flex items-center gap-2">
      <Button
        :label="t('kernel.check_update')" icon="pi pi-refresh" :loading="checking"
        severity="secondary" @click="onCheck"
      />
      <Button
        v-if="info?.latest && info.available" :label="t('kernel.upgrade_to', { version: info.latest })"
        icon="pi pi-download" :loading="installing === info.latest"
        @click="onInstall(info.latest)"
      />
      <span v-if="info && !info.available && info.latest" class="text-sm text-green-600">
        {{ t('kernel.up_to_date') }}
      </span>
    </div>

    <div v-if="info && info.versions.length > 0" class="flex flex-col gap-1">
      <span class="text-sm text-gray-500">{{ t('kernel.available_versions') }}</span>
      <div class="flex flex-wrap gap-2">
        <Button
          v-for="version in info.versions" :key="version"
          :label="version" size="small" outlined
          :severity="version === currentVersion ? 'success' : 'secondary'"
          :disabled="version === currentVersion"
          :loading="installing === version"
          @click="onInstall(version)"
        />
      </div>
    </div>
  </div>
</template>
