import Kit, { I18nUtils } from "@exvlan/std";

import { ConfirmationService, DialogService, ToastService } from "openvue";
import { createRouter, createWebHistory } from "vue-router";
import { routes } from "vue-router/auto-routes";

import App from "~/App.vue";
import { logDiagStep } from "~/composables/backend";
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
    // Tauri 的 invoke() 失败时 reject 的是**字符串**，不是 Error：
    // 必须补一个合成 stack，否则日志里只剩一句 message，找不到调用点。
    const message =
      reason instanceof Error
        ? reason.message
        : typeof reason === "string"
          ? reason
          : String(reason ?? "unhandled rejection");
    const stack =
      (reason instanceof Error ? reason.stack : undefined) ??
      `synthetic (reason type=${typeof reason})
${new Error("unhandledrejection").stack ?? ""}`;
    void reportFrontendError("fe:promise", message, stack);
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
