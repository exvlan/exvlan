import { invoke } from "@tauri-apps/api/core";

/**
 * UI 状态后端持久化。
 *
 * Android WebView 的 localStorage 不可靠（重启即丢），所以：
 *  - 启动最早期：`hydrateUiState()` 把后端文件里的键值回填到 localStorage
 *  - 之后所有 `localStorage.setItem` 都通过补丁**写穿**到后端（见 `installPersistHook`）
 *
 * 这样无需改动 std / 各业务模块里已有的写入代码。
 */

const nativeSet = Storage.prototype.setItem;
const nativeRemove = Storage.prototype.removeItem;

/** 后端 `prefs.toml` 是唯一权威：这些键启动时**无条件**覆盖 localStorage。 */
const AUTHORITATIVE_KEYS = new Set([
  "app_mode",
  "lang",
  "log_level",
  "last_network_instance_id",
  "diag_last_upload",
]);

/**
 * 从后端回填 localStorage（使用原生 setItem，避免触发写穿）。
 *
 * - 关键键（模式/语言/日志级别/上次网络）：**后端值覆盖前端**，避免「切了模式，
 *   重启又变回去」；
 * - 其它键：只补缺失，避免把 WebView 里更新但还没落盘的值冲掉。
 *
 * 后端不可用时会写 error.log（以前只 console.warn，排查不到）。
 */
export async function hydrateUiState(): Promise<void> {
  try {
    const state = await invoke<Record<string, string>>("get_ui_state");
    const restored: string[] = [];
    for (const [key, value] of Object.entries(state ?? {})) {
      const current = localStorage.getItem(key);
      const shouldWrite = current === null || (AUTHORITATIVE_KEYS.has(key) && current !== value);
      if (shouldWrite) {
        nativeSet.call(localStorage, key, value);
        restored.push(key);
      }
    }
    console.info("[prefs] hydrated:", restored.join(",") || "(none)");
  } catch (e) {
    console.warn("hydrateUiState failed:", e);
    void invoke("report_frontend_error", {
      scope: "fe:prefs",
      message: `hydrateUiState failed: ${String(e)}`,
    }).catch(() => {});
  }
}

/** 把一次写入推给后端（fire-and-forget，但失败会上报 error.log）。 */
function pushToBackend(key: string, value: string | null): void {
  void invoke("set_ui_state", { key, value }).catch((e) => {
    console.warn("set_ui_state failed:", key, e);
    void invoke("report_frontend_error", {
      scope: "fe:ui-state",
      message: `set_ui_state(${key}) failed: ${e}`,
    }).catch(() => {});
  });
}

/**
 * 关键设置的「写穿 + 等待」版本：切模式 / 记住上次网络这类值必须在退出前落盘，
 * 否则下次启动会被旧的后端值覆盖（历史 bug）。
 */
export async function persistUiStateNow(key: string, value: string | null): Promise<void> {
  nativeSet.call(localStorage, key, value ?? "");
  if (value === null) {
    nativeRemove.call(localStorage, key);
  }
  try {
    await invoke("set_ui_state", { key, value });
  } catch (e) {
    console.warn("set_ui_state(now) failed:", key, e);
    void invoke("report_frontend_error", {
      scope: "fe:ui-state",
      message: `set_ui_state(now) ${key} failed: ${e}`,
    }).catch(() => {});
  }
}

/**
 * 给 `localStorage.setItem/removeItem` 打补丁：先写本地，再写后端。
 * 必须在应用启动最早期调用一次。
 */
export function installPersistHook(): void {
  Storage.prototype.setItem = function (key: string, value: string) {
    nativeSet.call(this, key, value);
    pushToBackend(key, value);
  };
  Storage.prototype.removeItem = function (key: string) {
    nativeRemove.call(this, key);
    pushToBackend(key, null);
  };
}
