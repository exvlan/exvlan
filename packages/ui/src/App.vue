<script setup lang="ts">
import { I18nUtils } from '@exvlan/std'
import { onMounted } from 'vue';
import { Toast, DynamicDialog } from 'openvue';

onMounted(async () => {
  await I18nUtils.loadLanguageAsync(localStorage.getItem('lang') || 'en')
});

</script>

<!-- https://flowbite.com/docs/components/sidebar/#sidebar-with-navbar -->

<template>
  <Toast position="bottom-right" />
  <DynamicDialog />

  <RouterView />
</template>

<style scoped>
button {
  text-align: left;
  justify-content: left;
}
</style>
