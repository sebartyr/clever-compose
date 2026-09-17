// Déclarations d'ambiance : `@clevercloud/components` ne publie pas de
// types (web components chargés pour leurs effets de bord), et Vite
// expose `VITE_API_URL` au build.
declare module '@clevercloud/components/dist/*';

interface ImportMetaEnv {
  readonly VITE_API_URL?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
