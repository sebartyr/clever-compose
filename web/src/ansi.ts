// Rendu d'une ligne du journal : séquences ANSI SGR des logs applicatifs
// converties en couleurs, et verbe de nos propres lignes coloré comme dans
// le terminal. Le texte est échappé : le résultat va dans `v-html`.
const ANSI: Record<number, string> = { 30: '#555', 31: '#ff8a80', 32: '#8bd48b', 33: '#ffd54f', 34: '#82b1ff', 35: '#ea80fc', 36: '#80deea', 37: '#ddd', 90: '#999', 91: '#ff8a80', 92: '#8bd48b', 93: '#ffd54f', 94: '#82b1ff', 95: '#ea80fc', 96: '#80deea', 97: '#fff' };
const ESC = String.fromCharCode(27);
const SGR = new RegExp(ESC + '\\[([0-9;]*)m');
const esc = (t: string) => t.replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' } as Record<string, string>)[c]);

export function renderLine(line: string): string {
  if (line.includes(ESC + '[')) {
    let html = '';
    let style = '';
    const parts = line.split(SGR);
    for (let i = 0; i < parts.length; i++) {
      if (i % 2 === 0) { if (parts[i]) html += style ? `<span style="${style}">${esc(parts[i])}</span>` : esc(parts[i]); continue; }
      const codes = parts[i].split(';').map(Number);
      for (const c of codes) {
        if (c === 0) style = '';
        else if (c === 1) style += 'font-weight:bold;';
        else if (ANSI[c]) style += `color:${ANSI[c]};`;
      }
    }
    return html;
  }
  const m = line.match(/^([^|]+\| )?(.*)$/);
  const prefix = m?.[1] || '';
  const rest = m?.[2] ?? line;
  const has = (w: string[]) => w.some((x) => rest.includes(x));
  let color = '';
  if (rest.startsWith('erreur') || rest.includes('erreur [') || has(['échec', 'FAIL'])) color = ANSI[31];
  else if (rest.includes('avertissement [')) color = ANSI[33];
  else if (has(['supprimé', 'à supprimer'])) color = ANSI[31];
  else if (has(['existe', 'déjà', 'inchangé', 'sauté', 'conservé', 'ignoré'])) color = ANSI[90];
  else if (has(['attente', 'redéploiement', 'à créer', 'exécution locale', 'homonyme'])) color = ANSI[33];
  else if (has(['créé', 'déployée', 'lié', 'poussé', 'mis à jour', 'ajouté', 'terminé', 'présente', 'prêt', 'démarr', 'arrêtée (']) || rest.startsWith('✔')) color = ANSI[32];
  const sep = rest.indexOf(' : ');
  if (!prefix && sep > 0 && color) return `<b>${esc(rest.slice(0, sep))}</b> : <span style="color:${color}">${esc(rest.slice(sep + 3))}</span>`;
  return (prefix ? `<span style="color:${ANSI[36]}">${esc(prefix)}</span>` : '') + (color ? `<span style="color:${color}">${esc(rest)}</span>` : esc(rest));
}
