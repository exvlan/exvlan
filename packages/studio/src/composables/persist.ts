import { invoke } from '@tauri-apps/api/core'

/**
 * UI 状态后端持久化。
 *
 * Android WebView 的 localStorage 不可靠（重启即丢），所以：
 *  - 启动最早期：`hydrateUiState()` 把后端文件里的键值回填到 localStorage
 *  - 之后所有 `localStorage.setItem` 都通过补丁**写穿**到后端（见 `installPersistHook`）
 *
 * 这样无需改动 std / 各业务模块里已有的写入代码。
 */

const nativeSet = Storage.prototype.setItem
const nativeRemove = Storage.prototype.removeItem

/**
 * 从后端回填 localStorage（使用原生 setItem，避免触发写穿）。
 *
 * **只补缺失键**：后端文件可能是旧值（例如上次写穿没完成），
 * 若无条件覆盖，就会把 WebView 里较新的值（模式/上次网络）冲掉，
 * 表现为「切了模式，重启又变回去」。
 */
export async function hydrateUiState(): Promise<void> {
  try {
    const state = await invoke<Record<string, string>>('get_ui_state')
    const restored: string[] = []
    for (const [key, value] of Object.entries(state ?? {})) {
      if (localStorage.getItem(key) === null) {
        nativeSet.call(localStorage, key, value)
        restored.push(key)
      }
    }
    console.info('[ui-state] hydrated(missing only):', restored.join(',') || '(none)')
  }
  catch (e) {
    // 后端不可用（如 Web 控制台）时静默降级为纯 localStorage
    console.warn('hydrateUiState failed:', e)
  }
}

/** 把一次写入推给后端（fire-and-forget，但失败会上报 error.log）。 */
function pushToBackend(key: string, value: string | null): void {
  void invoke('set_ui_state', { key, value }).catch((e) => {
    console.warn('set_ui_state failed:', key, e)
    void invoke('report_frontend_error', {
      scope: 'fe:ui-state',
      message: `set_ui_state(${key}) failed: ${e}`,
    }).catch(() => {})
  })
}

/**
 * 关键设置的「写穿 + 等待」版本：切模式 / 记住上次网络这类值必须在退出前落盘，
 * 否则下次启动会被旧的后端值覆盖（历史 bug）。
 */
export async function persistUiStateNow(key: string, value: string | null): Promise<void> {
  nativeSet.call(localStorage, key, value ?? '')
  if (value === null) {
    nativeRemove.call(localStorage, key)
  }
  try {
    await invoke('set_ui_state', { key, value })
  }
  catch (e) {
    console.warn('set_ui_state(now) failed:', key, e)
    void invoke('report_frontend_error', {
      scope: 'fe:ui-state',
      message: `set_ui_state(now) ${key} failed: ${e}`,
    }).catch(() => {})
  }
}

/**
 * 给 `localStorage.setItem/removeItem` 打补丁：先写本地，再写后端。
 * 必须在应用启动最早期调用一次。
 */
export function installPersistHook(): void {
  Storage.prototype.setItem = function (key: string, value: string) {
    nativeSet.call(this, key, value)
    pushToBackend(key, value)
  }
  Storage.prototype.removeItem = function (key: string) {
    nativeRemove.call(this, key)
    pushToBackend(key, null)
  }
}
