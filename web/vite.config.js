// Mode empaqueté (`cargo build --features ui-bundled`) : tout est dans
// `dist/`, embarqué dans le binaire. Vue avec son compilateur de templates,
// puisque les composants sont écrits en templates de chaînes (pas de SFC :
// les mêmes sources tournent sans build, chargées depuis esm.sh).
import { defineConfig } from 'vite';

export default defineConfig({
  base: './',
  resolve: { alias: { vue: 'vue/dist/vue.esm-bundler.js' } },
  build: { outDir: 'dist', emptyOutDir: true, target: 'es2022' },
  server: { proxy: { '/api': 'http://127.0.0.1:7777' } },
});
