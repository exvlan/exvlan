import { createApp } from 'vue'
import '@exvlan/std/style.css'
import './style.css'
import App from './App.vue'
import ExvlanFrontendLib from '@exvlan/std'
import ConfirmationService from 'primevue/confirmationservice';

import { createRouter, createWebHashHistory } from 'vue-router'
import MainPage from './components/MainPage.vue'
import Login from './components/Login.vue'
import DeviceList from './components/DeviceList.vue'
import DeviceManagement from './components/DeviceManagement.vue'
import Dashboard from './components/Dashboard.vue'
import DialogService from 'primevue/dialogservice';
import ToastService from 'primevue/toastservice';
import ConfigGenerator from './components/ConfigGenerator.vue'

const routes = [
    {
        path: '/auth', children: [
            {
                name: 'login',
                path: '',
                component: Login,
                alias: 'login',
                props: { isRegistering: false }
            },
            {
                name: 'register',
                path: 'register',
                component: Login,
                props: { isRegistering: true }
            }
        ]
    },
    {
        path: '/h/:apiHost', component: MainPage, children: [
            {
                path: '',
                alias: 'dashboard',
                name: 'dashboard',
                component: Dashboard,
            },
            {
                path: 'deviceList',
                name: 'deviceList',
                component: DeviceList,
                children: [
                    {
                        path: 'device/:deviceId/:instanceId?',
                        name: 'deviceManagement',
                        component: DeviceManagement,
                    }
                ]
            },
        ]
    },
    {
        path: '/:pathMatch(.*)*', name: 'notFound', redirect: () => {
            let apiHost = localStorage.getItem('apiHost');
            if (apiHost) {
                return { name: 'dashboard', params: { apiHost: apiHost } }
            } else {
                return { name: 'login' }
            }
        }
    },
    {
        path: '/config_generator',
        component: ConfigGenerator,
    }
]

const router = createRouter({
    history: createWebHashHistory(),
    routes,
})

const app = createApp(App)

app.use(ToastService as any).use(DialogService as any).use(router).use(ConfirmationService as any).use(ExvlanFrontendLib).mount('#app')
