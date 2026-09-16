// Client de l'API locale de `clever-compose ui`. Les opérations longues
// arrivent en SSE (événements `log` puis `done`), y compris en POST : on lit
// le corps de la réponse `fetch` en flux.

async function json(method, url, body) {
  const res = await fetch(url, {
    method,
    headers: body ? { 'Content-Type': 'application/json' } : {},
    body: body ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  let data = {};
  try { data = text ? JSON.parse(text) : {}; } catch { data = { error: text }; }
  if (!res.ok) throw new Error(data.error || `${res.status} ${res.statusText}`);
  return data;
}

export const api = {
  info: () => json('GET', '/api/info'),
  session: () => json('GET', '/api/session'),
  setSession: (body) => json('POST', '/api/session', body),
  orgs: () => json('GET', '/api/orgs'),
  platformProjects: () => json('GET', '/api/platform/projects'),
  inventory: (project) => json('GET', `/api/platform/projects/${encodeURIComponent(project)}/inventory`),
  locals: () => json('GET', '/api/local'),
  addDir: (path) => json('POST', '/api/local', { path }),
  removeLocal: (id) => json('DELETE', `/api/local/${encodeURIComponent(id)}`),
  files: (id) => json('GET', `/api/local/${encodeURIComponent(id)}/files`),
  readFile: (id, path) => json('GET', `/api/local/${encodeURIComponent(id)}/file?path=${encodeURIComponent(path)}`),
  writeFile: (id, path, content) => json('PUT', `/api/local/${encodeURIComponent(id)}/file`, { path, content }),
  plan: (id, allowEphemeral) => json('GET', `/api/local/${encodeURIComponent(id)}/plan?allow_ephemeral=${allowEphemeral ? 'true' : 'false'}`),
  exportProject: (id, body) => json('POST', `/api/local/${encodeURIComponent(id)}/export`, body),

  async upload(file) {
    const form = new FormData();
    form.append('file', file, file.name);
    const res = await fetch('/api/local/upload', { method: 'POST', body: form });
    const data = await res.json();
    if (!res.ok) throw new Error(data.error || res.statusText);
    return data;
  },

  /** Lance une opération longue ; `onLog(line)` par ligne ; résout à la fin
   * (rejette sur `done: error: …`). Renvoie un contrôleur pour annuler. */
  stream(method, url, body, onLog) {
    const controller = new AbortController();
    const done = (async () => {
      const res = await fetch(url, {
        method,
        headers: body ? { 'Content-Type': 'application/json' } : {},
        body: body ? JSON.stringify(body) : undefined,
        signal: controller.signal,
      });
      if (!res.ok) {
        let msg = res.statusText;
        try { msg = (await res.json()).error || msg; } catch { /* corps non JSON */ }
        throw new Error(msg);
      }
      const reader = res.body.getReader();
      const decoder = new TextDecoder();
      let buffer = '';
      let event = 'message';
      let data = [];
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

  up: (id, body, onLog) => api.stream('POST', `/api/local/${encodeURIComponent(id)}/up`, body, onLog),
  seed: (id, body, onLog) => api.stream('POST', `/api/local/${encodeURIComponent(id)}/seed`, body, onLog),
  down: (project, body, onLog) => api.stream('POST', `/api/platform/projects/${encodeURIComponent(project)}/down`, body, onLog),
  logs: (project, params, onLog) => {
    const q = new URLSearchParams();
    if (params.services?.length) q.set('services', params.services.join(','));
    if (params.since) q.set('since', params.since);
    if (params.follow) q.set('follow', 'true');
    if (params.limit) q.set('limit', String(params.limit));
    return api.stream('GET', `/api/platform/projects/${encodeURIComponent(project)}/logs?${q}`, null, onLog);
  },
};
