import { createApp } from 'vue'
import Aura from '@openvue/themes/aura'
import ExvlanFrontendLib from '@exvlan/std'
import '@exvlan/std/style.css'
import PrimeVue from 'openvue/config'
import App from './App.vue'

createApp(App)
  .use(PrimeVue, {
    theme: {
      preset: Aura,
      options: {
        prefix: 'p',
        darkModeSelector: 'system',
        cssLayer: {
          name: 'primevue',
          order: 'tailwind-base, primevue, tailwind-utilities',
        },
      },
    },
  })
  .use(ExvlanFrontendLib)
  .mount('#app')
