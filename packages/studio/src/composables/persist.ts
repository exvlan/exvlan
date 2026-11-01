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

/** 从后端回填 localStorage（使用原生 setItem，避免触发写穿）。 */
export async function hydrateUiState(): Promise<void> {
  try {
    const state = await invoke<Record<string, string>>('get_ui_state')
    for (const [key, value] of Object.entries(state ?? {})) {
      nativeSet.call(localStorage, key, value)
    }
  }
  catch (e) {
    // 后端不可用（如 Web 控制台）时静默降级为纯 localStorage
    console.warn('hydrateUiState failed:', e)
  }
}

/** 把一次写入推给后端（fire-and-forget）。 */
function pushToBackend(key: string, value: string | null): void {
  void invoke('set_ui_state', { key, value }).catch((e) => {
    console.warn('set_ui_state failed:', key, e)
  })
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
