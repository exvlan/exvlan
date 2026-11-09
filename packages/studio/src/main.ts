import Kit, { I18nUtils } from "@exvlan/std";

import { ConfirmationService, DialogService, ToastService } from "openvue";
import { createRouter, createWebHistory } from "vue-router";
import { routes } from "vue-router/auto-routes";

import App from "~/App.vue";
import {
  legacyCleanupEnabled,
  logDiagStep,
  migrateNetworks,
  reportFrontendError,
} from "~/composables/backend";
import { hydrateUiState, installPersistHook } from "~/composables/persist";
import "@exvlan/std/style.css";
import "~/styles.css";

if (import.meta.env.PROD) {
  document.addEventListener("keydown", (event) => {
    if (
      event.key === "F5" ||
      (event.ctrlKey && event.key === "r") ||
      (event.metaKey && event.key === "r")
    ) {
      event.preventDefault();
    }
  });

  document.addEventListener("contextmenu", (event) => {
    event.preventDefault();
  });
}

/**
 * 一次性迁移：旧的 `localStorage.networkList` → `networks.toml`。
 * 成功后才删除 localStorage 副本。
 */
async function migrateLocalNetworks(): Promise<void> {
  try {
    const raw = localStorage.getItem("networkList");
    if (!raw) {
      return;
    }
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed) || parsed.length === 0) {
      return;
    }
    if (await migrateNetworks(raw)) {
      console.info("[networks] migrated localStorage → networks.toml");
    }
    // preview 默认保留快照；只有正式版（清理模式）才删除。
    if (await legacyCleanupEnabled().catch(() => false)) {
      localStorage.removeItem("networkList");
      console.info("[networks] stable release: localStorage snapshot removed");
    }
  } catch (e) {
    console.warn("migrate networks failed:", e);
    void reportFrontendError("fe:networks", `migrate failed: ${String(e)}`).catch(() => {});
  }
}

async function main() {
  // 启动阶段耗时打点（D5）：用于定位「IPC 未就绪 / hydrate 失败」这类竞态。
  const t0 = performance.now();
  const mark = (step: string) =>
    void logDiagStep(step, `${Math.round(performance.now() - t0)}ms`).catch(() => {});

  mark("boot:script_start");
  // 先回填后端持久化的 UI 状态（语言/模式/日志级别等），再初始化 i18n
  await hydrateUiState();
  mark("boot:hydrate_done");
  installPersistHook();
  await migrateLocalNetworks();

  await I18nUtils.loadLanguageAsync(localStorage.getItem("lang") || "en");
  mark("boot:i18n_ready");

  const app = createApp(App);

  // 所有未捕获异常都写进 <日志目录>/error.log，方便用户直接把日志发出来。
  window.addEventListener("error", (event) => {
    void reportFrontendError(
      "fe:window",
      event.message || String(event.error ?? "unknown error"),
      event.error instanceof Error ? event.error.stack : undefined,
    );
  });
  window.addEventListener("unhandledrejection", (event) => {
    const reason = event.reason;
    void reportFrontendError(
      "fe:promise",
      reason instanceof Error ? reason.message : String(reason ?? "unhandled rejection"),
      reason instanceof Error ? reason.stack : undefined,
    );
  });
  app.config.errorHandler = (err, _instance, info) => {
    console.error(err);
    void reportFrontendError(
      `fe:vue:${info}`,
      err instanceof Error ? err.message : String(err),
      err instanceof Error ? err.stack : undefined,
    );
  };

  const router = createRouter({
    history: createWebHistory(),
    routes,
  });

  app.use(router);
  app.use(createPinia());
  app.use(Kit);
  app.use(ToastService);
  app.use(DialogService);
  app.use(ConfirmationService);
  app.mount("#app");
  mark("boot:mounted");
}

main();
