import { type } from "@tauri-apps/plugin-os";

export interface WebClientConfig {
  config_server_url?: string;
}

export interface NormalMode extends WebClientConfig {
  mode: "normal";
  // if not provided will use ring tunnel rpc server
  rpc_portal?: string;
  enable_rpc_port_listen?: boolean;
  rpc_listen_port?: number;
}

export interface ServiceMode extends WebClientConfig {
  mode: "service";
  config_dir: string;
  rpc_portal: string;
  file_log_level: "off" | "warn" | "info" | "debug" | "trace";
  file_log_dir: string;
  installed_core_version?: string;
}

/**
 * 写回 `[app].mode`（S1 之后的形状：**只留现役设置** `config_server_url`）。
 *
 * - 运行方式由 `prefs.toml` 的 `runtime` 决定，不写在这里；
 * - `config_dir` / `rpc_portal` / `file_log_dir` / `file_log_level` 已由新路径推导，
 *   写回去只会污染 prefs（S1 判定为 pre.12 残留，运行时不再读）；
 * - `url` 为空 = **清除整个键**（与 `exvlan-migrate` 的 `keep: None` 同义）。
 *
 * 用「写穿 + 等待」：配置服务器地址必须在切模式/退出前真的落盘，
 * 否则下次启动 `loadMode()` 又读回旧值（历史 bug：改完只在内存里生效）。
 */
export async function saveConfigServerUrl(url?: string): Promise<void> {
  const { persistUiStateNow } = await import("./persist");
  const trimmed = (url ?? "").trim();
  if (!trimmed) {
    await persistUiStateNow("app_mode", null);
    return;
  }
  await persistUiStateNow("app_mode", JSON.stringify({ config_server_url: trimmed }));
}

export function loadMode(): Mode {
  const modeStr = localStorage.getItem("app_mode");
  if (modeStr) {
    const mode = JSON.parse(modeStr) as Mode;
    if (type() === "android") {
      return { ...mode, mode: "normal" };
    }
    return mode;
  } else {
    return { mode: "normal" };
  }
}

export type Mode = NormalMode | ServiceMode;
