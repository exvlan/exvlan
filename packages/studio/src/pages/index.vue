<script setup lang="ts">

import { type } from '@tauri-apps/plugin-os'

import { invoke } from '@tauri-apps/api/core'
import { writeText } from '@tauri-apps/plugin-clipboard-manager'
import { open } from '@tauri-apps/plugin-shell'
import { exit } from '@tauri-apps/plugin-process'
import { I18nUtils, RemoteManagement, Utils } from "@exvlan/std"
import type { MenuItem } from 'openvue/menuitem'
import { useTray } from '~/composables/tray'
import {
  consumePendingMobileVpnTileAction,
  initMobileVpnService,
  setMobileVpnTileActionHandler,
  syncMobileVpnService,
} from '~/composables/mobile_vpn'
import { executeVpnTileAction } from '~/composables/mobile_vpn_tile'
import { GUIRemoteClient } from '~/modules/api'

import { useToast, useConfirm } from 'openvue'
import { loadMode, saveMode, WebClientConfig, type Mode } from '~/composables/mode'
import { saveLastNetworkInstanceId, loadLastNetworkInstanceId } from '~/composables/config'
import ModeSwitcher from '~/components/ModeSwitcher.vue'
import KernelManager from '~/components/KernelManager.vue'
import { appDownloadUpdate, appInstallUpdate, appUpdateInfo, ensureKernelService, getServiceStatus } from '~/composables/backend'
import { appConfigDir, appLogDir, join } from '@tauri-apps/api/path'

const { t, locale } = useI18n()
const confirm = useConfirm()
const aboutVisible = ref(false)
const modeDialogVisible = ref(false)
const appUpdateDialogVisible = ref(false)
const appUpdateVersion = ref('')
const appUpdateNotes = ref('')
const appUpdating = ref(false)

const currentMode = ref<Mode>({ mode: 'normal' })
const editingMode = ref<Mode>({ mode: 'normal' })
const isModeSaving = ref(false)
const manualDisconnect = ref(false)

const configServerDialogVisible = ref(false)
const configServerConnected = ref(false)

const showAutostartHint = ref(false)

async function openModeDialog() {
  editingMode.value = JSON.parse(JSON.stringify(loadMode()))
  showAutostartHint.value = false
  modeDialogVisible.value = true
}

async function openAutostartDialog() {
  editingMode.value = JSON.parse(JSON.stringify(loadMode()))
  editingMode.value.mode = 'service'
  showAutostartHint.value = true
  modeDialogVisible.value = true
}

async function onModeSave() {
  if (isModeSaving.value) {
    return;
  }
  isModeSaving.value = true
  try {
    await initWithMode(editingMode.value);
    modeDialogVisible.value = false
  }
  catch (e: any) {
    toast.add({ severity: 'error', summary: t('error'), detail: e, life: 10000 })
    console.error("Error switching mode", e, currentMode.value, editingMode.value)
    await initWithMode(currentMode.value);
  void maybeCheckAppUpdate();
  }
  finally {
    isModeSaving.value = false
  }
}

async function onUninstallService() {
  confirm.require({
    message: t('mode.uninstall_service_confirm'),
    header: t('mode.uninstall_service'),
    icon: 'pi pi-exclamation-triangle',
    rejectProps: {
      label: t('web.common.cancel'),
      severity: 'secondary',
      outlined: true
    },
    acceptProps: {
      label: t('mode.uninstall_service'),
      severity: 'danger'
    },
    accept: async () => {
      isModeSaving.value = true
      try {
        await initWithMode({ ...currentMode.value, mode: 'normal' });
        await initService(undefined)
        toast.add({ severity: 'success', summary: t('web.common.success'), detail: t('mode.uninstall_service_success'), life: 3000 })
        modeDialogVisible.value = false
      } catch (e: any) {
        toast.add({ severity: 'error', summary: t('error'), detail: e, life: 10000 })
        console.error("Error uninstalling service", e)
      } finally {
        isModeSaving.value = false
      }
    },
  });
}

async function onStopService() {
  isModeSaving.value = true
  manualDisconnect.value = true
  try {
    await setServiceStatus(false)
    toast.add({ severity: 'success', summary: t('web.common.success'), detail: t('mode.stop_service_success'), life: 3000 })
    modeDialogVisible.value = false
  }
  catch (e: any) {
    toast.add({ severity: 'error', summary: t('error'), detail: e, life: 10000 })
    console.error("Error stopping service", e)
  }
  finally {
    isModeSaving.value = false
  }
}

async function initWithMode(mode: Mode) {
  const running_inst_ids = (await remoteClient.value.list_network_instance_ids().catch(() => undefined))?.running_inst_ids ?? []

  // 新架构：service 与 normal 共用同一个常驻 root 守护进程，
  // 区别仅在于「退出应用时是否停实例」。因此切模式**不再**停/卸载服务
  // （旧逻辑会因状态不匹配而失败，且会破坏 normal 模式所需的 TUN）。

  let url: string | undefined = undefined
  let retrys = 1
  switch (mode.mode) {
    case 'remote':
      if (!mode.remote_rpc_address) {
        toast.add({ severity: 'error', summary: t('error'), detail: t('mode.remote_rpc_address_empty'), life: 10000 })
        return initWithMode({ ...mode, mode: 'normal' });
      }
      url = mode.remote_rpc_address
      break;
    case 'service': {
      if (!mode.config_dir || !mode.file_log_dir || !mode.file_log_level || !mode.rpc_portal) {
        toast.add({ severity: 'error', summary: t('error'), detail: t('mode.service_config_empty'), life: 10000 })
        return initWithMode({ ...mode, mode: 'normal' });
      }
      // 是否（重）装服务只由**服务指纹**决定：内核路径 + 启动参数完全一致时
      // ensureKernelService 直接返回 current，不会触发授权（历史 bug：用版本号比较，
      // 而 exvlan_version 现在返回内核版本，导致每次启动都判定需要重装 → 反复弹密码）。
      mode.config_server_url = mode.config_server_url || undefined
      await ensureKernelService({
        config_dir: mode.config_dir,
        file_log_dir: mode.file_log_dir,
        file_log_level: mode.file_log_level,
        rpc_portal: mode.rpc_portal,
        config_server: mode.config_server_url,
      })
      let serviceStatus = await getServiceStatus()
      if (serviceStatus === "Stopped") {
        await setServiceStatus(true)
      }
      url = "tcp://" + mode.rpc_portal.replace("0.0.0.0", "127.0.0.1")
      retrys = 5
      break;
    }
    case 'normal':
      url = mode.rpc_portal;
      break;
  }
  for (let i = 0; i < retrys; i++) {
    try {
      await connectRpcClient(mode.mode === 'normal', url)
      break;
    } catch (e) {
      if (i === retrys - 1) {
        const errMsg = e instanceof Error ? e.message : String(e)
        toast.add({
          severity: 'error',
          summary: t('error'),
          detail: t('mode.rpc_connection_failed', { error: errMsg }),
          life: 1000,
        })
        throw e;
      }
      console.error("Error connecting rpc client, retrying...", e)
      await new Promise(resolve => setTimeout(resolve, 1000))
    }
  }
  await sendConfigs(running_inst_ids.map(Utils.UuidToStr))
  if (mode.mode === 'normal') {
    mode.config_server_url = mode.config_server_url || undefined
    initWebClient(mode.config_server_url)
  }
  currentMode.value = mode
  saveMode(mode)
  clientRunning.value = await isClientRunning()
}

/** 桌面端启动闭环：确保内核已安装、服务定义与当前配置一致（一致时不提权）。 */
async function ensureLocalKernel(mode: Mode) {
  const platform = type()
  if (platform === 'android' || platform === 'ios')
    return
  if (mode.mode === 'remote')
    return

  // daemon 参数与模式无关：优先取当前 mode 上的字段（service 模式有），
  // 否则回退到已保存的 app_mode 里的同名字段，最后才用默认值。
  const saved = loadMode() as { config_dir?: string, rpc_portal?: string, file_log_level?: string, file_log_dir?: string }
  const merged = {
    config_dir: (mode as any).config_dir || saved.config_dir || await join(await appConfigDir(), 'config.d'),
    rpc_portal: (mode as any).rpc_portal || saved.rpc_portal || '127.0.0.1:15999',
    file_log_level: (mode as any).file_log_level || saved.file_log_level || 'info',
    file_log_dir: (mode as any).file_log_dir || saved.file_log_dir || await appLogDir(),
    config_server: (mode as { config_server_url?: string }).config_server_url,
  }
  await ensureKernelService(merged)
}

/** 应用更新：启动静默检查（24h 节流，已忽略的版本不再打扰）。 */
async function maybeCheckAppUpdate() {
  const KEY_LAST = 'upgrade_last_check'
  const KEY_IGNORED = 'upgrade_ignored_version'

  const last = Number(localStorage.getItem(KEY_LAST) || 0)
  if (Date.now() - last < 24 * 3600 * 1000) {
    return
  }

  try {
    const info = await appUpdateInfo()
    localStorage.setItem(KEY_LAST, String(Date.now()))
    if (!info.available || !info.latest) {
      return
    }
    if (localStorage.getItem(KEY_IGNORED) === info.latest) {
      return
    }
    appUpdateVersion.value = info.latest
    appUpdateNotes.value = info.notes || ''
    appUpdateDialogVisible.value = true
  }
  catch (e) {
    // 更新源未同步/离线：静默
    console.warn('app update check failed', e)
  }
}

async function onAppUpdateNow() {
  if (!appUpdateVersion.value) {
    return
  }
  appUpdating.value = true
  try {
    const path = await appDownloadUpdate(appUpdateVersion.value)
    await appInstallUpdate(path)
    appUpdateDialogVisible.value = false
  }
  catch (e: any) {
    toast.add({ severity: 'error', summary: t('error'), detail: String(e), life: 10000 })
  }
  finally {
    appUpdating.value = false
  }
}

function onAppUpdateLater() {
  localStorage.setItem('upgrade_ignored_version', appUpdateVersion.value)
  appUpdateDialogVisible.value = false
}

onMounted(async () => {
  const cleanupFns: Array<() => void> = []

  if (type() === 'android') {
    try {
      await initMobileVpnService()
    } catch (e: any) {
      console.error("exvlan init vpn service failed", e)
    }
  }

  cleanupFns.push(await listenGlobalEvents())
  currentMode.value = loadMode()

  // 首次安装/升级后会自动装好内核并（重新）注册系统服务；失败不阻塞进入界面。
  try {
    await ensureLocalKernel(currentMode.value)
  } catch (e: any) {
    console.error("ensure kernel/service failed", e)
    toast.add({ severity: 'error', summary: t('error'), detail: String(e), life: 10000 })
  }

  await initWithMode(currentMode.value);
  void maybeCheckAppUpdate();

  if (type() === 'android') {
    setMobileVpnTileActionHandler(handleMobileVpnTileAction)
    cleanupFns.push(() => setMobileVpnTileActionHandler())
    try {
      await consumePendingMobileVpnTileAction()
      await syncMobileVpnService()
    } catch (e: any) {
      console.error("exvlan sync vpn service failed", e)
    }
  }

  onUnmounted(() => {
    cleanupFns.forEach(unlisten => unlisten())
  })
});

useTray(true)
let toast = useToast();

const remoteClient = computed(() => new GUIRemoteClient());
const instanceId = ref<string | undefined>(undefined);
const clientRunning = ref(false);

async function handleMobileVpnTileAction(action: 'start' | 'stop') {
  try {
    const result = await executeVpnTileAction(action, remoteClient.value, {
      lastInstanceId: loadLastNetworkInstanceId(),
      syncVpnService: syncMobileVpnService,
    })

    if (!result.instanceId) {
      toast.add({
        severity: 'warn',
        summary: t('vpn_tile_no_network'),
        detail: t('vpn_tile_no_network_description'),
        life: 5000,
      })
      return
    }

    instanceId.value = result.instanceId
    saveLastNetworkInstanceId(result.instanceId)
    toast.add({
      severity: action === 'start' ? 'success' : 'secondary',
      summary: t(action === 'start' ? 'vpn_tile_started' : 'vpn_tile_stopped'),
      life: 3000,
    })
  }
  catch (error) {
    console.error('VPN tile action failed', action, error)
    toast.add({
      severity: 'error',
      summary: t('error'),
      detail: t('vpn_tile_action_failed', { error: String(error) }),
      life: 8000,
    })
  }
}

watch(instanceId, (newVal) => {
  if (newVal) {
    saveLastNetworkInstanceId(newVal);
  }
});

watch(clientRunning, async (newVal, oldVal) => {
  if (!newVal && oldVal) {
    if (manualDisconnect.value) {
      manualDisconnect.value = false
      return
    }
    await reconnectClient()
  } else if (newVal && !oldVal) {
    const lastInstanceId = loadLastNetworkInstanceId();
    if (lastInstanceId) {
      instanceId.value = lastInstanceId;
    }
  }
})

onMounted(async () => {
  clientRunning.value = await isClientRunning().catch(() => false)
  const timer = setInterval(async () => {
    try {
      clientRunning.value = await isClientRunning()
    } catch (e) {
      clientRunning.value = false
      console.error("Error checking client running status", e)
    }
  }, 1000)

  onUnmounted(() => {
    clearInterval(timer)
  })
})
async function reconnectClient() {
  editingMode.value = JSON.parse(JSON.stringify(loadMode()));
  await onModeSave()
}

onMounted(async () => {
  window.setTimeout(async () => {
    await setTrayMenu([
      await MenuItemShow(t('tray.show')),
      await MenuItemExit(t('tray.exit')),
    ])
  }, 1000)
})

let current_log_level = 'off'

const log_menu = ref()
// 从后端获取正确的日志路径
async function getLogDirPath(): Promise<string> {
  return await invoke<string>('get_log_dir_path')
}

const log_menu_items_popup: Ref<MenuItem[]> = ref([
  ...['off', 'warn', 'info', 'debug', 'trace'].map(level => ({
    label: () => t(`logging_level_${level}`) + (current_log_level === level ? ' ✓' : ''),
    command: async () => {
      current_log_level = level
      await setLoggingLevel(level)
    },
  })),
  {
    separator: true,
  },
  {
    label: () => t('logging_open_dir'),
    icon: 'pi pi-folder-open',
    command: async () => {
      // console.log('open log dir', await getLogDirPath())
      await open(await getLogDirPath())
    },
    visible: () => type() !== 'android',
  },
  {
    label: () => t('logging_copy_dir'),
    icon: 'pi pi-tablet',
    command: async () => {
      await writeText(await getLogDirPath())
    },
  },
])

function toggle_log_menu(event: any) {
  log_menu.value.toggle(event)
}

function getLabel(item: MenuItem) {
  return typeof item.label === 'function' ? item.label() : item.label
}

const setting_menu_items: Ref<MenuItem[]> = ref([
  {
    label: () => t('exchange_language'),
    icon: 'pi pi-language',
    command: async () => {
      await I18nUtils.loadLanguageAsync((locale.value === 'en' ? 'cn' : 'en'))
      await setTrayMenu([
        await MenuItemShow(t('tray.show')),
        await MenuItemExit(t('tray.exit')),
      ])
    },
  },
  {
    label: () => `${t('mode.switch_mode')}: ${t('mode.' + currentMode.value.mode)}`,
    icon: 'pi pi-sync',
    command: openModeDialog,
    visible: () => type() !== 'android',
  },
  {
    label: () => t('mode.autostart'),
    icon: 'pi pi-clock',
    command: openAutostartDialog,
    visible: () => type() !== 'android',
  },
  {
    label: () => `${t('config-server.title')}${t('config-server.' + configServerConnectionStatus.value)}`,
    icon: 'pi pi-globe',
    command: openConfigServerDialog,
    visible: () => ["normal", "service"].includes(currentMode.value.mode),
  },
  {
    key: 'logging_menu',
    label: () => t('logging'),
    icon: 'pi pi-file',
    items: [], // Keep this to show it's a parent menu
  },
  {
    label: () => t('about.title'),
    icon: 'pi pi-at',
    command: async () => {
      aboutVisible.value = true
    },
  },
  {
    label: () => t('exit'),
    icon: 'pi pi-power-off',
    command: async () => {
      await exit(1)
    },
  },
])

async function connectRpcClient(isNormalMode: boolean, url?: string) {
  await initRpcConnection(isNormalMode, url)
  console.log("exvlan rpc connection established, isNormalMode: ", isNormalMode)
}

async function openConfigServerDialog() {
  editingMode.value = JSON.parse(JSON.stringify(loadMode()))
  configServerDialogVisible.value = true
}
async function onConfigServerSave() {
  if (JSON.stringify(currentMode.value) === JSON.stringify(editingMode.value)) {
    configServerDialogVisible.value = false
    return;
  }
  if (editingMode.value.mode === 'service') {
    await new Promise<void>((resolve, reject) => {
      confirm.require({
        message: t('config-server.update_service_confirm'),
        icon: 'pi pi-exclamation-triangle',
        rejectProps: {
          label: t('web.common.cancel'),
          severity: 'secondary',
          outlined: true
        },
        acceptProps: {
          label: t('web.common.confirm'),
        },
        accept: async () => {
          resolve()
        },
        reject: () => {
          reject()
        }
      });
    })
  }
  console.log("Saving config server url", (editingMode.value as WebClientConfig).config_server_url)
  await onModeSave();
  configServerDialogVisible.value = false
}
onMounted(() => {
  const timer = setInterval(async () => {
    if (currentMode.value.mode !== 'normal') return;
    if (!currentMode.value.config_server_url) return;
    configServerConnected.value = await isWebClientConnected();
  }, 1000)

  onUnmounted(() => {
    clearInterval(timer)
  })
})
const configServerConnectionStatus = computed(() => {
  if (currentMode.value.mode !== 'normal') {
    return 'unknown'
  }
  if (!currentMode.value.config_server_url) {
    return 'disconnected'
  }
  return configServerConnected.value ? 'connected' : 'connecting'
})

</script>

<template>
  <div id="root" class="flex flex-col">
    <Dialog v-model:visible="aboutVisible" modal :header="t('about.title')" :style="{ width: '70%' }">
      <About />
    </Dialog>

    <Dialog v-model:visible="appUpdateDialogVisible" modal :header="t('upgrade.app_version')" :style="{ width: '40vw' }">
      <div class="flex flex-col gap-2">
        <p>{{ t('upgrade.new_version', { version: appUpdateVersion }) }}</p>
        <p v-if="appUpdateNotes" class="text-sm whitespace-pre-wrap text-gray-500">{{ appUpdateNotes }}</p>
      </div>
      <template #footer>
        <Button :label="t('web.common.cancel')" text @click="onAppUpdateLater" />
        <Button :label="t('upgrade.download_and_install')" :loading="appUpdating" @click="onAppUpdateNow" />
      </template>
    </Dialog>
    <Dialog v-model:visible="modeDialogVisible" modal :header="t('mode.switch_mode')" :style="{ width: '50vw' }">
      <Message v-if="showAutostartHint" severity="info" :closable="false" class="mb-4">
        {{ t('mode.autostart_hint') }}
      </Message>
      <ModeSwitcher v-model="editingMode" @uninstall-service="onUninstallService" @stop-service="onStopService" />
      <KernelManager v-if="type() !== 'android' && type() !== 'ios'" class="mt-4" />
      <template #footer>
        <Button :label="t('web.common.cancel')" icon="pi pi-times" @click="modeDialogVisible = false" text />
        <Button :label="t('web.common.save')" icon="pi pi-save" @click="onModeSave" autofocus :loading="isModeSaving" />
      </template>
    </Dialog>

    <Dialog v-model:visible="configServerDialogVisible" modal :header="t('config-server.title')"
      :style="{ width: '50vw' }">
      <div class="flex flex-col gap-3">
        <label for="config-server-address">{{ t('config-server.address') }}</label>
        <InputText id="config-server-address" v-model="(editingMode as WebClientConfig).config_server_url"
          :placeholder="t('config-server.address_placeholder')" />
        <small class="p-text-secondary whitespace-pre-wrap">{{ t('config-server.description') }}</small>
      </div>
      <template #footer>
        <Button :label="t('web.common.cancel')" icon="pi pi-times" @click="configServerDialogVisible = false" text />
        <Button :label="t('web.common.save')" icon="pi pi-save" @click="onConfigServerSave" autofocus
          :loading="isModeSaving" />
      </template>
    </Dialog>

    <Menu ref="log_menu" :model="log_menu_items_popup" :popup="true" />

    <RemoteManagement v-if="clientRunning" class="flex-1 overflow-y-auto" :api="remoteClient"
      :pause-auto-refresh="isModeSaving" v-model:instance-id="instanceId" />
    <div v-else class="empty-state flex-1 flex flex-col items-center py-12">
      <i class="pi pi-server text-5xl text-secondary mb-4 opacity-50"></i>
      <div class="text-xl text-center font-medium mb-3">{{ t('client.not_running') }}
      </div>
      <Button @click="reconnectClient" :loading="isModeSaving" :label="t('client.retry')" icon="pi pi-replay"
        iconPos="left" />
    </div>

    <Menubar :model="setting_menu_items" breakpoint="795px">
      <template #item="{ item, props }">
        <a v-if="item.key === 'logging_menu'" v-bind="props.action" @click="toggle_log_menu">
          <span :class="item.icon" />
          <span class="p-menubar-item-label">{{ getLabel(item) }}</span>
          <span class="pi pi-angle-down p-menubar-item-icon text-[9px]"></span>
        </a>
        <a v-else v-bind="props.action">
          <span :class="item.icon" />
          <span class="p-menubar-item-label">{{ getLabel(item) }}</span>
        </a>
      </template>
    </Menubar>
  </div>
</template>

<style scoped lang="postcss">
#root {
  height: 100vh;
  width: 100vw;
}

.p-dropdown :deep(.p-dropdown-panel .p-dropdown-items .p-dropdown-item) {
  padding: 0 0.5rem;
}
</style>

<style>
body {
  height: 100vh;
  width: 100vw;
  padding: 0;
  margin: 0;
  overflow: hidden;
}

.p-menubar .p-menuitem {
  margin: 0;
}

.p-select-overlay {
  max-width: calc(100% - 2rem);
}

/*

.p-tabview-panel {
  height: 100%;
} */
</style>
