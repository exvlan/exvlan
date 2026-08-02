import Kit, { I18nUtils } from '@exvlan/kit'
import Aura from '@primeuix/themes/aura'

import { ConfirmationService, DialogService, ToastService } from 'primevue'
import PrimeVue from 'primevue/config'
import { createRouter, createWebHistory } from 'vue-router'
import { routes } from 'vue-router/auto-routes'

import App from '~/App.vue'
import '@exvlan/kit/style.css'
import '~/styles.css'

if (import.meta.env.PROD) {
  document.addEventListener('keydown', (event) => {
    if (
      event.key === 'F5'
      || (event.ctrlKey && event.key === 'r')
      || (event.metaKey && event.key === 'r')
    ) {
      event.preventDefault()
    }
  })

  document.addEventListener('contextmenu', (event) => {
    event.preventDefault()
  })
}

async function main() {
  await I18nUtils.loadLanguageAsync(localStorage.getItem('lang') || 'en')

  const app = createApp(App)

  const router = createRouter({
    history: createWebHistory(),
    routes,
  })

  app.use(router)
  app.use(createPinia())
  app.use(Kit)
  // app.use(i18n, { useScope: 'global' })
  app.use(PrimeVue, {
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
  app.use(ToastService)
  app.use(DialogService)
  app.use(ConfirmationService)
  app.mount('#app')
}

main()
