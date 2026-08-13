import './style.css'

import type { App } from 'vue';
import { Config, Status, ConfigEditDialog, RemoteManagement } from "./components";
// 使用 aura-compat（14px 基准校准）而非标准 aura（16px 基准）：
// 应用 root font-size 为 12px（studio/std）与 0.9rem≈14.4px（ui），
// 标准 aura 会让控件按 16px 缩放（12px root 下文字仅 10.5px），
// compat 预设保持 v4 时代外观（1rem = 与应用正文一致）。
import Aura from '@primeuix/themes/aura-compat';
import PrimeVue from 'primevue/config'

import I18nUtils from './modules/i18n'
import * as NetworkTypes from './types/network'
import HumanEvent from './components/HumanEvent.vue';

// do not use primevue tooltip, it has serious memory leak issue
// https://github.com/primefaces/primevue/issues/5856
// import Tooltip from 'primevue/tooltip';
import { vTooltip } from 'floating-vue';

import * as Api from './modules/api';
import * as Utils from './modules/utils';

export default {
    install: (app: App): void => {
        app.use(I18nUtils.i18n, { useScope: 'global' })
        app.use(PrimeVue, {
            theme: {
                preset: Aura,
                options: {
                    prefix: 'p',
                    darkModeSelector: 'system',
                    cssLayer: {
                        name: 'primevue',
                        order: 'tailwind-base, primevue, tailwind-utilities'
                    }
                },
            },
            zIndex: {
                modal: 1100,        //dialog, drawer
                overlay: 1200,      //select, popover
                menu: 1300,         //overlay menus
                tooltip: 1400       //tooltip
            },
            license: import.meta.env.VITE_PRIMEUI_LICENSE
        });

        app.component('Config', Config);
        app.component('ConfigEditDialog', ConfigEditDialog);
        app.component('Status', Status);
        app.component('HumanEvent', HumanEvent);
        app.component('RemoteManagement', RemoteManagement);
        app.directive('tooltip', vTooltip as any);
    }
};

export { Config, ConfigEditDialog, RemoteManagement, Status, I18nUtils, NetworkTypes, Api, Utils };
