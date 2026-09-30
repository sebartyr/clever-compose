// Client de l'API locale de `clever-compose ui`. Les opérations longues
// arrivent en SSE (événements `log` puis `done`), y compris en POST : on lit
// le corps de la réponse `fetch` en flux.
import type { AddonBody, AddonPlans, AdoptResult, Assets, Drift, ExportResult, Flavors, Info, Inventory, Json, LocalProject, Org, Plan, Diagnostic, ScalingBody, Session } from './types';

export type OnLog = (line: string) => void;
/** Une opération en cours : sa fin, et de quoi l'interrompre. */
export interface Stream {
  done: Promise<void>;
  cancel: () => void;
}
export interface Tokens {
  token: string;
  secret: string;
  email?: string;
}

// URL de l'API : VITE_API_URL au build (interface déployée à part), sinon
// la même origine que la page (interface servie par `clever-compose ui`).
export const API_BASE = (import.meta.env.VITE_API_URL || '').replace(/\/$/, '');
const u = (path: string) => API_BASE + path;

// Clé d'API du serveur (`--api-key`), gardée pour l'onglet (sessionStorage)
// et envoyée en `Authorization: Bearer` ; sans clé configurée côté serveur,
// rien n'est envoyé.
let apiKey = '';
try { apiKey = sessionStorage.getItem('clever-compose.api-key') || ''; } catch { /* stockage indisponible */ }
export const hasApiKey = () => !!apiKey;
export const setApiKey = (k: string) => { apiKey = k || ''; try { if (apiKey) sessionStorage.setItem('clever-compose.api-key', apiKey); else sessionStorage.removeItem('clever-compose.api-key'); } catch { /* stockage indisponible */ } };
// Jetons Clever Cloud de l'utilisateur (danse OAuth faite par ce
// navigateur), gardés pour l'onglet et envoyés à chaque requête : le
// serveur agit en son nom. Sans jetons, le serveur utilise les siens s'il
// en a (usage local). L'organisation choisie part en `X-CC-Org`.
let tokens: Tokens | null = null;
try { tokens = JSON.parse(sessionStorage.getItem('clever-compose.tokens') || 'null'); } catch { /* stockage indisponible */ }
export const hasTokens = () => !!(tokens && tokens.token && tokens.secret);
export const setTokens = (t: Partial<Tokens> | null) => { tokens = t && t.token && t.secret ? { token: t.token, secret: t.secret, email: t.email || '' } : null; try { if (tokens) sessionStorage.setItem('clever-compose.tokens', JSON.stringify(tokens)); else sessionStorage.removeItem('clever-compose.tokens'); } catch { /* stockage indisponible */ } };
export const tokensEmail = () => (tokens && tokens.email) || '';
let org = '';
try { org = localStorage.getItem('clever-compose.org') || ''; } catch { /* stockage indisponible */ }
export const currentOrg = () => org;
export const setOrg = (o: string) => { org = o || ''; try { if (org) localStorage.setItem('clever-compose.org', org); else localStorage.removeItem('clever-compose.org'); } catch { /* stockage indisponible */ } };
const headers = (body?: unknown): Record<string, string> => ({
  ...(body ? { 'Content-Type': 'application/json' } : {}),
  ...(apiKey ? { Authorization: 'Bearer ' + apiKey } : {}),
  ...(tokens && tokens.token && tokens.secret ? { 'X-CC-Token': tokens.token, 'X-CC-Secret': tokens.secret } : {}),
  ...(org ? { 'X-CC-Org': org } : {}),
});

async function json<T = Json>(method: string, url: string, body?: unknown): Promise<T> {
  const res = await fetch(u(url), {
    method,
    headers: headers(body),
    body: body ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  let data: Json = {};
  try { data = text ? JSON.parse(text) : {}; } catch { data = { error: text }; }
  if (!res.ok) throw new Error(data.error || `${res.status} ${res.statusText}`);
  return data as T;
}

export const api = {
  info: () => json<Info>('GET', '/api/info'),
  session: () => json<Session>('GET', '/api/session'),
  orgs: () => json<{ orgs: Org[] }>('GET', '/api/orgs'),
  platformProjects: () => json<{ projects: string[] }>('GET', '/api/platform/projects'),
  inventory: (project: string) => json<Inventory>('GET', `/api/platform/projects/${encodeURIComponent(project)}/inventory`),
  assets: (project: string) => json<Assets>('GET', `/api/platform/projects/${encodeURIComponent(project)}/assets`),
  assetsOpen: (project: string) => json<{ id: string }>('POST', `/api/platform/projects/${encodeURIComponent(project)}/assets/open`),
  locals: () => json<{ projects: LocalProject[] }>('GET', '/api/local'),
  addDir: (path: string) => json<{ id: string }>('POST', '/api/local', { path }),
  removeLocal: (id: string) => json('DELETE', `/api/local/${encodeURIComponent(id)}`),
  files: (id: string) => json<{ id: string; dir: string; name: string; files: string[] }>('GET', `/api/local/${encodeURIComponent(id)}/files`),
  readFile: (id: string, path: string) => json<{ path: string; content: string }>('GET', `/api/local/${encodeURIComponent(id)}/file?path=${encodeURIComponent(path)}`),
  writeFile: (id: string, path: string, content: string) => json('PUT', `/api/local/${encodeURIComponent(id)}/file`, { path, content }),
  plan: (id: string, allowEphemeral: boolean) => json<{ plan: Plan | null; diagnostics: Diagnostic[] }>('GET', `/api/local/${encodeURIComponent(id)}/plan?allow_ephemeral=${allowEphemeral ? 'true' : 'false'}`),
  exportProject: (id: string, body: { branches: boolean; allow_ephemeral: boolean }) => json<ExportResult>('POST', `/api/local/${encodeURIComponent(id)}/export`, body),
  flavors: () => json<Flavors>('GET', '/api/flavors'),
  setScaling: (id: string, body: ScalingBody) => json('PUT', `/api/local/${encodeURIComponent(id)}/scaling`, body),
  addonPlans: () => json<AddonPlans>('GET', '/api/addon-plans'),
  setAddon: (id: string, body: AddonBody) => json('PUT', `/api/local/${encodeURIComponent(id)}/addon`, body),
  drifts: (id: string) => json<{ drifts: Drift[] }>('GET', `/api/local/${encodeURIComponent(id)}/drift`),
  adoptDrifts: (id: string, ids: string[]) => json<AdoptResult>('POST', `/api/local/${encodeURIComponent(id)}/drift`, { ids }),
  syncAssets: (id: string, onLog: OnLog) => api.stream('POST', `/api/local/${encodeURIComponent(id)}/assets/sync`, null, onLog),
  /** Connexion : rend { mode: 'redirect', url }. */
  // Retour après la danse : relatif quand l'API sert la page (même
  // origine), absolu quand elle est ailleurs (VITE_API_URL, origine à
  // passer en --cors au serveur).
  login: () => json<{ mode: string; url: string }>('POST', '/api/session/login', { return: API_BASE ? location.origin + location.pathname : location.pathname }),

  async uploadDump(id: string, file: File): Promise<{ path: string; size: number }> {
    const form = new FormData();
    form.append('file', file, file.name);
    const res = await fetch(u(`/api/local/${encodeURIComponent(id)}/seed/upload`), { method: 'POST', headers: headers(), body: form });
    const data = await res.json();
    if (!res.ok) throw new Error(data.error || res.statusText);
    return data;
  },

  async upload(file: File): Promise<{ id: string }> {
    const form = new FormData();
    form.append('file', file, file.name);
    const res = await fetch(u('/api/local/upload'), { method: 'POST', headers: headers(), body: form });
    const data = await res.json();
    if (!res.ok) throw new Error(data.error || res.statusText);
    return data;
  },

  /** Lance une opération longue ; `onLog(line)` par ligne ; résout à la fin
   * (rejette sur `done: error: …`). Renvoie un contrôleur pour annuler. */
  stream(method: string, url: string, body: unknown, onLog: OnLog): Stream {
    const controller = new AbortController();
    const done = (async (): Promise<void> => {
      const res = await fetch(u(url), {
        method,
        headers: headers(body),
        body: body ? JSON.stringify(body) : undefined,
        signal: controller.signal,
      });
      if (!res.ok) {
        let msg = res.statusText;
        try { msg = (await res.json()).error || msg; } catch { /* corps non JSON */ }
        throw new Error(msg);
      }
      const reader = res.body!.getReader();
      const decoder = new TextDecoder();
      let buffer = '';
      let event = 'message';
      let data: string[] = [];
      for (;;) {
        const { value, done: end } = await reader.read();
        if (end) break;
        buffer += decoder.decode(value, { stream: true });
        let idx;
        while ((idx = buffer.indexOf('\n')) >= 0) {
          const line = buffer.slice(0, idx).replace(/\r$/, '');
          buffer = buffer.slice(idx + 1);
          if (line === '') {
            if (data.length) {
              const payload = data.join('\n');
              if (event === 'done') {
                if (payload.startsWith('error: ')) throw new Error(payload.slice(7));
                return;
              }
              onLog(payload);
            }
            event = 'message';
            data = [];
          } else if (line.startsWith('event:')) {
            event = line.slice(6).trim();
          } else if (line.startsWith('data:')) {
            data.push(line.slice(5).replace(/^ /, ''));
          }
        }
      }
    })();
    return { done, cancel: () => controller.abort() };
  },

  up: (id: string, body: { yes: boolean; dry_run: boolean; allow_ephemeral: boolean; no_wait: boolean; assets: boolean; parallel: number }, onLog: OnLog) => api.stream('POST', `/api/local/${encodeURIComponent(id)}/up`, body, onLog),
  seed: (id: string, body: { service: string; dump: string; local: boolean; yes: boolean; no_wait: boolean }, onLog: OnLog) => api.stream('POST', `/api/local/${encodeURIComponent(id)}/seed`, body, onLog),
  stop: (project: string, onLog: OnLog) => api.stream('POST', `/api/platform/projects/${encodeURIComponent(project)}/stop`, null, onLog),
  start: (project: string, onLog: OnLog) => api.stream('POST', `/api/platform/projects/${encodeURIComponent(project)}/start`, null, onLog),
  down: (project: string, body: { yes: boolean; dry_run: boolean }, onLog: OnLog) => api.stream('POST', `/api/platform/projects/${encodeURIComponent(project)}/down`, body, onLog),
  logs: (project: string, params: { services?: string[]; since?: string; follow?: boolean; limit?: number }, onLog: OnLog) => {
    const q = new URLSearchParams();
    if (params.services?.length) q.set('services', params.services.join(','));
    if (params.since) q.set('since', params.since);
    if (params.follow) q.set('follow', 'true');
    if (params.limit) q.set('limit', String(params.limit));
    return api.stream('GET', `/api/platform/projects/${encodeURIComponent(project)}/logs?${q}`, null, onLog);
  },
};
