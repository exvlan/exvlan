import { defineConfig } from "vitest/config";

// 本地最小配置：避免 vitest 上溯到仓库根的 vite-plus 配置（sdk 为独立 workspace，无该依赖）。
export default defineConfig({});
