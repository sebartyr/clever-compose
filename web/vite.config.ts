// Build de l'interface : tout est dans `dist/`, embarqué dans le binaire
// par le `build.rs` de la CLI. Les composants sont des SFC compilés ici :
// aucun compilateur de templates dans le bundle. Les balises `cc-*` sont
// des web components (@clevercloud/components), pas des composants Vue.
import { defineConfig } from 'vite';
import vue from '@vitejs/plugin-vue';

export default defineConfig({
  base: './',
  plugins: [vue({ template: { compilerOptions: { isCustomElement: (tag) => tag.startsWith('cc-') } } })],
  build: { outDir: 'dist', emptyOutDir: true, target: 'es2022' },
  // Proxy de développement vers `clever-compose api` : le serveur vérifie
  // `Host` et `Origin` (anti-CSRF) ; on lui présente les siens, la page et
  // le proxy étant la même origine pour le navigateur.
  server: {
    proxy: {
      '/api': {
        target: 'http://127.0.0.1:8080',
        changeOrigin: true,
        configure: (proxy) => proxy.on('proxyReq', (req) => req.removeHeader('origin')),
      },
    },
  },
});
