<script setup lang="ts">
import { getCurrentWindow } from "@tauri-apps/api/window";
// 显式引入：全局只挂一个确认组件（popup），避免与页面内 ConfirmDialog 重复渲染。
import { ConfirmPopup } from "openvue";
import pkg from "~/../package.json";

onBeforeMount(async () => {
  await getCurrentWindow().setTitle(`Exvlan Studio: v${pkg.version}`);
});
</script>

<template>
  <Toast position="bottom-right" />
  <ConfirmPopup />
  <RouterView />
</template>
