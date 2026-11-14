// `[app].mode` 的读取/写回（S1 之后的形状：只留 `config_server_url`）。
//
// 这里同时覆盖「真的落盘」与「模拟重启后仍读得回」：`persistUiStateNow` 写穿到
// `set_ui_state` → `prefs.toml`，启动时 `hydrateUiState` 再回填 localStorage。
// Node 环境没有 `Storage`/`localStorage`，用最小内存实现顶上（在 `vi.hoisted`
// 里定义，保证 `persist.ts` 模块级 `Storage.prototype.setItem` 取到的是它）。
import { beforeEach, describe, expect, it, vi } from "vite-plus/test";

const env = vi.hoisted(() => {
  const backend = new Map<string, string>();

  class MemoryStorage {
    private readonly map = new Map<string, string>();
    get length(): number {
      return this.map.size;
    }

    clear(): void {
      this.map.clear();
    }

    getItem(key: string): string | null {
      return this.map.get(key) ?? null;
    }

    key(index: number): string | null {
      return [...this.map.keys()][index] ?? null;
    }

    removeItem(key: string): void {
      this.map.delete(key);
    }

    setItem(key: string, value: string): void {
      this.map.set(key, String(value));
    }
  }

  const localStorage = new MemoryStorage();
  Object.assign(globalThis, { Storage: MemoryStorage, localStorage });
  return { backend, localStorage };
});

vi.mock("@tauri-apps/plugin-os", () => ({ type: () => "linux" }));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(async (cmd: string, args?: Record<string, unknown>) => {
    // 后端 `prefs.toml` 的最小等价物：`value: null` = 删键（prefs.rs::set_key）。
    if (cmd === "get_ui_state") return Object.fromEntries(env.backend);
    if (cmd === "set_ui_state") {
      const key = String(args?.key);
      const value = args?.value;
      if (value === null || value === undefined) env.backend.delete(key);
      else env.backend.set(key, String(value));
    }
    return undefined;
  }),
}));

const { loadMode, saveConfigServerUrl } = await import("./mode");
const { hydrateUiState } = await import("./persist");

beforeEach(() => {
  env.backend.clear();
  env.localStorage.clear();
});

describe("config server 地址的持久化", () => {
  it("写回 [app].mode，且只留 config_server_url（不带 pre.12 残留字段）", async () => {
    await saveConfigServerUrl("  tcp://cf.example:2018  ");

    const stored = env.localStorage.getItem("app_mode");
    expect(stored).not.toBeNull();
    const parsed = JSON.parse(stored ?? "{}") as Record<string, unknown>;
    // 地址两端空白被裁掉；目录/端口/运行方式都不写回（见 mode.ts 注释）
    expect(parsed).toEqual({ config_server_url: "tcp://cf.example:2018" });
    expect(env.backend.get("app_mode")).toBe(stored);
  });

  it("模拟重启：清空前端缓存后 hydrateUiState 仍读回最后保存的值", async () => {
    await saveConfigServerUrl("tcp://first:2018");
    await saveConfigServerUrl("tcp://second:2018");

    // 重启 = 前端 localStorage 全丢，只剩 prefs.toml
    env.localStorage.clear();
    expect(env.localStorage.getItem("app_mode")).toBeNull();

    await hydrateUiState();

    expect(loadMode().config_server_url).toBe("tcp://second:2018");
  });

  it("清空地址 = 删掉整个 app_mode 键，重启后回到未配置状态", async () => {
    await saveConfigServerUrl("tcp://first:2018");
    await saveConfigServerUrl("");

    expect(env.backend.has("app_mode")).toBe(false);
    expect(env.localStorage.getItem("app_mode")).toBeNull();

    await hydrateUiState();

    // 没有 app_mode → loadMode() 的缺省值，`config_server_url` 为 undefined
    expect(loadMode()).toEqual({ mode: "normal" });
    expect(loadMode().config_server_url).toBeUndefined();
  });
});
