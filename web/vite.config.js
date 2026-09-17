// Build de l'interface : tout est dans `dist/`, embarqué dans le binaire
// par le `build.rs` de la CLI. Vue avec son compilateur de templates,
// puisque les composants sont écrits en templates de chaînes.
import { defineConfig } from 'vite';

export default defineConfig({
  base: './',
  resolve: { alias: { vue: 'vue/dist/vue.esm-bundler.js' } },
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
