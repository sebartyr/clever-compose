// Point d'entrée : composants Clever Cloud (traductions en français),
// application Vue montée sur #app. Les balises `cc-*` sont des web
// components, pas des composants Vue.
import { createApp } from 'vue';
import { addTranslations, setLanguage } from '@clevercloud/components/dist/i18n.js';
import { lang, translations } from '@clevercloud/components/dist/translations.fr.js';
import '@clevercloud/components/dist/cc-badge.js';
import '@clevercloud/components/dist/cc-block.js';
import '@clevercloud/components/dist/cc-button.js';
import '@clevercloud/components/dist/cc-dialog.js';
import '@clevercloud/components/dist/cc-input-text.js';
import '@clevercloud/components/dist/cc-loader.js';
import '@clevercloud/components/dist/cc-notice.js';
import '@clevercloud/components/dist/cc-select.js';
import '@clevercloud/components/dist/cc-toggle.js';
import { App } from './app.js';

addTranslations(lang, translations);
setLanguage(lang);

const app = createApp(App);
app.config.compilerOptions.isCustomElement = (tag) => tag.startsWith('cc-');
app.mount('#app');
