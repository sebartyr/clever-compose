// Valeur d'un événement de formulaire : `detail` des web components
// (`cc-select`, `cc-input-text`, `cc-toggle`), sinon `value` de la cible.
export const val = (e: Event | undefined): any => (e && 'detail' in e && (e as CustomEvent).detail !== undefined ? (e as CustomEvent).detail : (e?.target as HTMLInputElement | null)?.value);
