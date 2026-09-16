// L'application : projets locaux et projets sur la plateforme, diagramme,
// détail d'une ressource, éditeur du compose, diagnostics, exports, journal
// des opérations. Toutes les commandes de la CLI sont ici.
import { computed, defineComponent, onMounted, reactive, ref, watch } from 'vue';
import { api } from './api.js';
import { Diagram } from './diagram.js';
import { graphFromInventory, graphFromPlan } from './graph.js';

const val = (e) => (e && e.detail !== undefined ? e.detail : e?.target?.value);

// ------------------------------------------------------------ session

const SessionPanel = defineComponent({
  name: 'SessionPanel',
  props: { session: Object, orgs: Array },
  emits: ['changed'],
  setup(props, { emit }) {
    const open = ref(false);
    const form = reactive({ api_token: '', token: '', secret: '' });
    const busy = ref(false);
    const status = ref('');
    const error = ref('');
    const submit = async () => {
      busy.value = true; error.value = '';
      try {
        await api.setSession({ api_token: form.api_token, token: form.token, secret: form.secret });
        form.api_token = ''; form.token = ''; form.secret = '';
        open.value = false;
        emit('changed');
      } catch (e) { error.value = e.message; } finally { busy.value = false; }
    };
    const chooseOrg = async (e) => {
      const org = val(e);
      if (!org) return;
      busy.value = true;
      try { await api.setSession({ org }); emit('changed'); } catch (err) { error.value = err.message; } finally { busy.value = false; }
    };
    return { open, form, busy, error, submit, chooseOrg, val };
  },
  template: `
    <div class="session">
      <template v-if="session?.authenticated">
        <cc-select label="Organisation" :hiddenLabel="true" :inline="true" :options="orgs.map(o => ({ label: o.name, value: o.id }))" :value="session.org" placeholder="Choisir une organisation" @cc-select="chooseOrg" :disabled="busy"></cc-select>
        <cc-button :outlined="true" title="Changer les identifiants" @cc-click="open = true">Jetons</cc-button>
      </template>
      <cc-button v-else :primary="true" title="Saisir un jeton d'API ou OAuth 1.0" @cc-click="open = true">Se connecter</cc-button>
      <cc-dialog heading="Identifiants Clever Cloud" :open="open" @cc-close="open = false" closedBy="any">
        <p class="hint">Un jeton d'API (bearer), ou un jeton OAuth 1.0 et son secret (ceux de la CLI clever conviennent). Ils restent en mémoire du serveur local, jamais sur le disque.</p>
        <cc-input-text label="Jeton d'API" :secret="true" :value="form.api_token" @cc-input="e => form.api_token = val(e)"></cc-input-text>
        <p class="hint">— ou —</p>
        <cc-input-text label="Jeton OAuth 1.0" :secret="true" :value="form.token" @cc-input="e => form.token = val(e)"></cc-input-text>
        <cc-input-text label="Secret OAuth 1.0" :secret="true" :value="form.secret" @cc-input="e => form.secret = val(e)"></cc-input-text>
        <cc-notice v-if="error" intent="danger" :message="error"></cc-notice>
        <div class="row-end">
          <cc-button :outlined="true" @cc-click="open = false">Annuler</cc-button>
          <cc-button :primary="true" :waiting="busy" @cc-click="submit">Vérifier et utiliser</cc-button>
        </div>
      </cc-dialog>
    </div>
  `,
});

// ------------------------------------------------------------ journal

// Rendu d'une ligne : séquences ANSI SGR des logs applicatifs converties en
// couleurs, et verbe de nos propres lignes coloré comme dans le terminal.
const ANSI = { 30: '#555', 31: '#ff8a80', 32: '#8bd48b', 33: '#ffd54f', 34: '#82b1ff', 35: '#ea80fc', 36: '#80deea', 37: '#ddd', 90: '#999', 91: '#ff8a80', 92: '#8bd48b', 93: '#ffd54f', 94: '#82b1ff', 95: '#ea80fc', 96: '#80deea', 97: '#fff' };
const esc = (t) => t.replace(/[&<>]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;' })[c]);
function renderLine(line) {
  if (line.includes('\u001b[')) {
    let html = '';
    let style = '';
    const parts = line.split(/\u001b\[([0-9;]*)m/);
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
  const prefix = m[1] || '';
  const rest = m[2];
  const has = (w) => w.some((x) => rest.includes(x));
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

const LogPanel = defineComponent({
  name: 'LogPanel',
  props: { lines: Array, running: Boolean, title: String },
  emits: ['cancel', 'clear'],
  setup(props) {
    const el = ref(null);
    watch(() => props.lines.length, () => { requestAnimationFrame(() => { if (el.value) el.value.scrollTop = el.value.scrollHeight; }); });
    return { el, renderLine };
  },
  template: `
    <section class="logs" v-if="lines.length || running">
      <header>
        <strong>{{ title || 'Journal' }}</strong>
        <cc-loader v-if="running" a11y-name="en cours"></cc-loader>
        <span class="grow"></span>
        <cc-button v-if="running" :outlined="true" :danger="true" title="Interrompt l'opération en cours" @cc-click="$emit('cancel')">Arrêter</cc-button>
        <cc-button v-else :outlined="true" title="Vide le journal" @cc-click="$emit('clear')">Effacer</cc-button>
      </header>
      <pre ref="el"><div v-for="(l, i) in lines" :key="i" v-html="renderLine(l)"></div></pre>
    </section>
  `,
});

// ------------------------------------------------------------ détail d'un nœud

const Detail = defineComponent({
  name: 'NodeDetail',
  props: { node: Object, platform: Boolean },
  emits: ['close', 'logs'],
  setup(props) {
    const env = computed(() => (props.node?.data?.env || []).map((e) => ({ key: e.key, value: e.value.map((s) => s.literal !== undefined ? s.literal : `\${${s.ref.var || s.ref.member || s.ref.name}}`).join('') })));
    const json = computed(() => JSON.stringify(props.node?.data || {}, null, 2));
    return { env, json };
  },
  template: `
    <aside class="detail" v-if="node">
      <header>
        <cc-badge :weight="'dimmed'" :intent="node.kind === 'addon' ? 'info' : 'neutral'">{{ node.kind }}</cc-badge>
        <strong>{{ node.label }}</strong>
        <span class="grow"></span>
        <cc-button :outlined="true" :hideText="true" title="Fermer le détail" @cc-click="$emit('close')">Fermer</cc-button>
      </header>
      <p class="sub">{{ node.sub }}</p>
      <template v-if="platform && node.kind !== 'addon'">
        <dl>
          <dt>Identifiant</dt><dd><code>{{ node.data.id }}</code></dd>
          <dt>Instances</dt><dd>{{ node.data.instances.length ? node.data.instances.map(i => i.state.toLowerCase() + ' (' + (i.flavor || '?') + ')').join(', ') : 'aucune' }}</dd>
          <dt>Dernier déploiement</dt><dd>{{ node.data.last_deployment ? node.data.last_deployment.state : 'aucun' }}</dd>
          <dt v-if="node.data.domains.length">Domaines</dt><dd v-if="node.data.domains.length"><a v-for="d in node.data.domains" :key="d" :href="'https://' + d" target="_blank" rel="noopener">{{ d }}</a></dd>
          <dt>Tags</dt><dd>{{ node.data.tags.join(', ') || '—' }}</dd>
        </dl>
        <cc-button :outlined="true" @cc-click="$emit('logs', node.label)">Logs de {{ node.label }}</cc-button>
      </template>
      <template v-else-if="platform">
        <dl>
          <dt>Identifiant</dt><dd><code>{{ node.data.id }}</code></dd>
          <dt>Id réel</dt><dd><code>{{ node.data.real_id }}</code></dd>
          <dt>Fournisseur</dt><dd>{{ node.data.provider }} · plan {{ node.data.plan }}</dd>
        </dl>
        <cc-button :outlined="true" @cc-click="$emit('logs', node.label)">Logs de {{ node.label }}</cc-button>
      </template>
      <template v-else-if="node.kind !== 'addon'">
        <dl>
          <dt>Nom sur la plateforme</dt><dd><code>{{ node.data.name }}</code></dd>
          <dt>Région</dt><dd>{{ node.data.region }}</dd>
          <dt>Commande</dt><dd><code>{{ node.data.shim.argv.join(' ') }}</code></dd>
          <dt>Port</dt><dd>{{ node.data.shim.port.mode }}{{ node.data.shim.port.target_port ? ' → ' + node.data.shim.port.target_port : '' }}</dd>
          <dt v-if="node.data.depends_on.length">Dépend de</dt><dd v-if="node.data.depends_on.length">{{ node.data.depends_on.join(', ') }}</dd>
        </dl>
        <h4 v-if="env.length">Environnement</h4>
        <div v-if="env.length" class="env"><div v-for="e in env" :key="e.key" class="var"><code class="k">{{ e.key }}</code><code class="v">{{ e.value }}</code></div></div>
      </template>
      <template v-else>
        <dl>
          <dt>Nom sur la plateforme</dt><dd><code>{{ node.data.name }}</code></dd>
          <dt>Fournisseur</dt><dd>{{ node.data.provider }}{{ node.data.options?.documentdb ? ' + DocumentDB' : '' }}</dd>
          <dt>Plan</dt><dd>{{ node.data.plan }}{{ node.data.version ? ' · version ' + node.data.version : '' }}</dd>
          <dt>Accès</dt><dd>{{ node.data.private ? 'network group' : 'hôte public' }}</dd>
        </dl>
      </template>
      <details><summary>JSON</summary><pre class="json">{{ json }}</pre></details>
    </aside>
  `,
});

// ------------------------------------------------------------ application

export const App = defineComponent({
  name: 'CleverComposeApp',
  components: { Diagram, SessionPanel, LogPanel, Detail },
  setup() {
    const session = ref({ authenticated: false, org: '' });
    const info = ref({});
    const orgs = ref([]);
    const locals = ref([]);
    const platformProjects = ref([]);
    const current = ref(null); // { type: 'local'|'platform', id, name }
    const tab = ref('diagram');
    const plan = ref(null);
    const diagnostics = ref([]);
    const inventory = ref(null);
    const viewPlatform = ref(false);
    const files = ref([]);
    const file = reactive({ path: '', content: '', dirty: false });
    const exportResult = ref(null);
    const exportFile = ref('');
    const selected = ref(null);
    const busy = ref(false);
    const error = ref('');
    const logs = reactive({ lines: [], running: false, title: '', cancel: null, progress: false });
    const opts = reactive({ yes: false, dry_run: false, allow_ephemeral: false, no_wait: false, branches: false });
    const seed = reactive({ service: '', dump: '', local: false, uploading: false });
    const uploadDump = async (e) => {
      const f = e.target.files?.[0];
      if (!f) return;
      seed.uploading = true;
      try { const r = await api.uploadDump(current.value.id, f); seed.dump = r.path; files.value = (await api.files(current.value.id)).files; } catch (err) { notify(err); } finally { seed.uploading = false; e.target.value = ''; }
    };
    const logQuery = reactive({ services: '', since: '1h', follow: true });
    const dialog = reactive({ down: false, seed: false, addDir: false, path: '' });

    const notify = (e) => { error.value = e?.message || String(e); setTimeout(() => { if (error.value === (e?.message || String(e))) error.value = ''; }, 8000); };

    const loadInfo = async () => { try { info.value = await api.info(); } catch { /* serveur absent */ } };
    const refreshSession = async () => {
      session.value = await api.session();
      if (session.value.authenticated) {
        try { orgs.value = (await api.orgs()).orgs; } catch (e) { notify(e); }
        if (session.value.org) await refreshPlatform();
      }
    };
    const refreshLocals = async () => { locals.value = (await api.locals()).projects; };
    const refreshPlatform = async () => {
      if (!session.value.org) { platformProjects.value = []; return; }
      try { platformProjects.value = (await api.platformProjects()).projects; } catch (e) { notify(e); }
    };

    const graph = computed(() => {
      if (current.value?.type === 'platform' || viewPlatform.value) return inventory.value ? graphFromInventory(inventory.value) : { nodes: [], groups: [], edges: [] };
      return plan.value ? graphFromPlan(plan.value) : { nodes: [], groups: [], edges: [] };
    });
    const selectedNode = computed(() => graph.value.nodes.find((n) => n.id === selected.value) || null);
    const errors = computed(() => diagnostics.value.filter((d) => d.severity === 'error').length);
    const onPlatform = computed(() => current.value && platformProjects.value.includes(current.value.name));

    const loadPlan = async () => {
      if (current.value?.type !== 'local') return;
      busy.value = true; status.value = 'Calcul du plan (lecture des images sur leurs registres)…';
      try {
        const r = await api.plan(current.value.id, opts.allow_ephemeral);
        plan.value = r.plan; diagnostics.value = r.diagnostics;
        const errs = r.diagnostics.filter((d) => d.severity === 'error').length;
        status.value = errs ? `Plan refusé : ${errs} erreur(s), voyez les diagnostics` : `Plan à jour : ${r.plan.resources.length} ressources, ${r.diagnostics.length} avertissement(s)`;
        setTimeout(() => { if (status.value.startsWith('Plan')) status.value = ''; }, 6000);
      } catch (e) { status.value = ''; notify(e); } finally { busy.value = false; }
    };
    const loadInventory = async () => {
      if (!current.value || !session.value.org) return;
      try { inventory.value = await api.inventory(current.value.name); } catch (e) { inventory.value = null; if (current.value.type === 'platform') notify(e); }
    };
    const openLocal = async (p) => {
      current.value = { type: 'local', id: p.id, name: p.name };
      selected.value = null; viewPlatform.value = false; exportResult.value = null; inventory.value = null;
      tab.value = 'diagram';
      const f = await api.files(p.id);
      files.value = f.files;
      current.value.name = f.name;
      const compose = f.files.find((x) => /compose\.ya?ml$/.test(x)) || f.files[0];
      if (compose) await openFile(compose);
      await loadPlan();
      await loadInventory();
    };
    const openPlatform = async (name) => {
      current.value = { type: 'platform', id: name, name };
      selected.value = null; plan.value = null; diagnostics.value = []; exportResult.value = null;
      tab.value = 'diagram';
      await loadInventory();
    };
    const openFile = async (path) => {
      const r = await api.readFile(current.value.id, path);
      file.path = r.path; file.content = r.content; file.dirty = false;
    };
    const saveFile = async () => {
      await api.writeFile(current.value.id, file.path, file.content);
      file.dirty = false;
      await loadPlan();
    };

    const run = (title, starter) => {
      logs.lines = []; logs.running = true; logs.title = title;
      // Une ligne de progression (préfixe \u0001, temps écoulé) remplace la
      // précédente au lieu de s'empiler.
      const ctl = starter((line) => {
        if (line.startsWith('\u0001')) {
          const text = line.slice(1);
          if (logs.progress && logs.lines.length) logs.lines[logs.lines.length - 1] = text; else logs.lines.push(text);
          logs.progress = true;
        } else { logs.lines.push(line); logs.progress = false; }
      });
      logs.cancel = ctl.cancel;
      ctl.done.then(() => { logs.lines.push('✔ terminé'); }).catch((e) => { logs.lines.push('erreur : ' + e.message); }).finally(async () => {
        logs.running = false; logs.cancel = null;
        await refreshPlatform();
        await loadInventory();
        if (current.value?.type === 'local') viewPlatform.value = !!inventory.value;
      });
    };
    const doUp = () => run('up ' + current.value.name, (onLog) => api.up(current.value.id, { yes: opts.yes, dry_run: opts.dry_run, allow_ephemeral: opts.allow_ephemeral, no_wait: opts.no_wait }, onLog));
    const doStop = () => run('stop ' + current.value.name, (onLog) => api.stop(current.value.name, onLog));
    const doStart = () => run('start ' + current.value.name, (onLog) => api.start(current.value.name, onLog));
    const doDown = () => { dialog.down = false; run('down ' + current.value.name, (onLog) => api.down(current.value.name, { yes: opts.yes, dry_run: opts.dry_run }, onLog)); };
    const doSeed = () => { dialog.seed = false; run('seed ' + seed.service, (onLog) => api.seed(current.value.id, { service: seed.service, dump: seed.dump, local: seed.local, yes: opts.yes, no_wait: opts.no_wait }, onLog)); };
    const doLogs = (service) => {
      const services = service ? [service] : logQuery.services.split(',').map((s) => s.trim()).filter(Boolean);
      run('logs ' + current.value.name + (services.length ? ' ' + services.join(' ') : ''), (onLog) => api.logs(current.value.name, { services, since: logQuery.since, follow: logQuery.follow }, onLog));
    };
    const doExport = async () => {
      busy.value = true;
      try {
        exportResult.value = await api.exportProject(current.value.id, { branches: opts.branches, allow_ephemeral: opts.allow_ephemeral });
        exportFile.value = exportResult.value.files[0]?.path || '';
        tab.value = 'export';
      } catch (e) { notify(e); } finally { busy.value = false; }
    };
    const upload = async (e) => {
      const f = e.target.files?.[0];
      if (!f) return;
      try { const r = await api.upload(f); await refreshLocals(); const p = locals.value.find((x) => x.id === r.id); if (p) await openLocal(p); } catch (err) { notify(err); }
      e.target.value = '';
    };
    const addDir = async () => {
      try { const r = await api.addDir(dialog.path); dialog.addDir = false; dialog.path = ''; await refreshLocals(); const p = locals.value.find((x) => x.id === r.id); if (p) await openLocal(p); } catch (e) { notify(e); }
    };
    const removeLocal = async (p) => { await api.removeLocal(p.id); if (current.value?.id === p.id) current.value = null; await refreshLocals(); };

    watch(() => opts.allow_ephemeral, loadPlan);
    onMounted(async () => {
      await loadInfo(); await refreshSession(); await refreshLocals();
      // Liens directs : ?project=<nom> (plateforme) ou ?local=<id>.
      const q = new URLSearchParams(location.search);
      if (q.get('project') && session.value.org) await openPlatform(q.get('project'));
      else if (q.get('local') && locals.value.some((p) => p.id === q.get('local'))) await openLocal(locals.value.find((p) => p.id === q.get('local')));
      else if (locals.value.length) await openLocal(locals.value[0]);
    });

    return { status, info, session, orgs, locals, platformProjects, current, tab, plan, diagnostics, inventory, viewPlatform, files, file, exportResult, exportFile, selected, selectedNode, busy, error, logs, opts, seed, logQuery, dialog, graph, errors, onPlatform, refreshSession, refreshLocals, refreshPlatform, openLocal, openPlatform, openFile, saveFile, loadPlan, loadInventory, doUp, doDown, doStop, doStart, doSeed, uploadDump, doLogs, doExport, upload, addDir, removeLocal, val };
  },
  template: `
    <div class="layout">
      <header class="top">
        <h1>clever-compose <small class="dim" :title="'build ' + (info.built || '') + (info.bundled ? ', interface empaquetée' : ', interface via CDN')">{{ info.version }}{{ info.commit ? ' · ' + info.commit : '' }}</small></h1>
        <span class="grow"></span>
        <session-panel :session="session" :orgs="orgs" @changed="refreshSession"></session-panel>
      </header>
      <nav class="side">
        <h2>Projets locaux</h2>
        <ul>
          <li v-for="p in locals" :key="p.id" :class="{ active: current?.id === p.id }">
            <a href="#" @click.prevent="openLocal(p)">{{ p.name }}</a>
            <span class="dim" :title="p.dir">{{ p.temporary ? 'archive' : p.dir }}</span>
            <cc-button :outlined="true" :hideText="true" :danger="true" title="Retire le projet de la liste (une archive extraite est supprimée du disque)" @cc-click="removeLocal(p)">Retirer</cc-button>
          </li>
        </ul>
        <label class="upload" title="Archive .zip, .tar.gz, .tgz ou .tar contenant le projet ; extraite dans un répertoire temporaire"><input type="file" accept=".zip,.tar,.tgz,.tar.gz" @change="upload"><span>Charger une archive (.zip, .tar.gz)</span></label>
        <cc-button :outlined="true" title="Un répertoire du poste qui contient un compose.yaml" @cc-click="dialog.addDir = true">Ouvrir un répertoire du poste</cc-button>
        <h2>Sur la plateforme <cc-button :outlined="true" :hideText="true" title="Relit les projets de l'organisation" @cc-click="refreshPlatform">Rafraîchir</cc-button></h2>
        <p v-if="!session.authenticated" class="dim">Connectez-vous pour voir les projets déployés.</p>
        <p v-else-if="!session.org" class="dim">Choisissez une organisation.</p>
        <ul v-else>
          <li v-for="name in platformProjects" :key="name" :class="{ active: current?.type === 'platform' && current.id === name }"><a href="#" @click.prevent="openPlatform(name)">{{ name }}</a></li>
          <li v-if="!platformProjects.length" class="dim">aucun projet clever-compose</li>
        </ul>
      </nav>
      <main class="main">
        <cc-notice v-if="error" intent="danger" :message="error" :closeable="true" @cc-notice-dismiss="error = ''"></cc-notice>
        <template v-if="!current">
          <cc-block><div slot="header-title">Bienvenue</div><div slot="content">Chargez une archive, ouvrez un répertoire contenant un <code>compose.yaml</code>, ou choisissez un projet déployé.</div></cc-block>
        </template>
        <template v-else>
          <section class="toolbar">
            <h2>{{ current.name }} <cc-badge v-if="current.type === 'local'" weight="dimmed">local</cc-badge><cc-badge v-if="onPlatform" intent="success" weight="dimmed">déployé</cc-badge></h2>
            <span class="grow"></span>
            <template v-if="current.type === 'local'">
              <span class="options">
                <label class="switch" title="Confirme la création des addons (facturés) ; sans elle, up s'arrête avant de créer un addon"><input type="checkbox" v-model="opts.yes"> --yes</label>
                <label class="switch" title="Décrit ce qui serait fait sans rien modifier sur la plateforme"><input type="checkbox" v-model="opts.dry_run"> --dry-run</label>
                <label class="switch" title="Accepte les volumes nommés des services non substituables comme stockage éphémère (données perdues au redéploiement)"><input type="checkbox" v-model="opts.allow_ephemeral"> --allow-ephemeral</label>
                <label class="switch" title="N'attend pas la fin des déploiements"><input type="checkbox" v-model="opts.no_wait"> --no-wait</label>
              </span>
              <cc-button :outlined="true" :waiting="busy" :disabled="busy" title="Recalcule le plan et les diagnostics, sans rien créer" @cc-click="loadPlan">Plan</cc-button>
              <cc-button :primary="true" :disabled="errors > 0 || logs.running" title="Crée ou met à jour les ressources et déploie (--yes confirme les addons, facturés)" @cc-click="doUp">Up</cc-button>
              <cc-button :outlined="true" :disabled="!plan || logs.running" title="Charge un dump local dans un addon, une fois par contenu" @cc-click="dialog.seed = true">Seed</cc-button>
              <cc-button :outlined="true" :disabled="errors > 0" title="Terraform (JSON et HCL) et script clever-tools" @cc-click="doExport">Export</cc-button>
            </template>
            <cc-button v-if="onPlatform || current.type === 'platform'" :outlined="true" :disabled="logs.running" title="Logs des applications et addons du projet" @cc-click="doLogs()">Logs</cc-button>
            <cc-button v-if="onPlatform || current.type === 'platform'" :outlined="true" :disabled="logs.running" title="Arrête les applications sans rien supprimer (comme docker compose stop)" @cc-click="doStop">Stop</cc-button>
            <cc-button v-if="onPlatform || current.type === 'platform'" :outlined="true" :disabled="logs.running" title="Relance les applications arrêtées" @cc-click="doStart">Start</cc-button>
            <cc-button v-if="onPlatform || current.type === 'platform'" :danger="true" :outlined="true" :disabled="logs.running" title="Supprime les applications et network groups ; les addons seulement avec --yes" @cc-click="dialog.down = true">Down</cc-button>
          </section>
          <p v-if="status" class="status"><cc-loader v-if="busy" a11y-name="en cours"></cc-loader> {{ status }}</p>
          <nav class="tabs">
            <a :class="{ active: tab === 'diagram' }" href="#" @click.prevent="tab = 'diagram'">Diagramme</a>
            <a v-if="current.type === 'local'" :class="{ active: tab === 'compose' }" href="#" @click.prevent="tab = 'compose'">Compose</a>
            <a v-if="current.type === 'local'" :class="{ active: tab === 'diagnostics' }" href="#" @click.prevent="tab = 'diagnostics'">Diagnostics <cc-badge v-if="diagnostics.length" :intent="errors ? 'danger' : 'warning'" weight="dimmed" circle>{{ diagnostics.length }}</cc-badge></a>
            <a v-if="exportResult" :class="{ active: tab === 'export' }" href="#" @click.prevent="tab = 'export'">Export</a>
            <span class="grow"></span>
            <label v-if="current.type === 'local' && onPlatform" class="switch" title="Diagramme de ce qui existe sur la plateforme (instances, déploiements) au lieu du plan"><input type="checkbox" v-model="viewPlatform" @change="loadInventory"> état sur la plateforme</label>
            <cc-button v-if="current.type === 'platform'" :outlined="true" title="Relit l'état des ressources sur la plateforme" @cc-click="loadInventory">Rafraîchir</cc-button>
          </nav>
          <div class="content" v-show="tab === 'diagram'">
            <div class="canvas">
              <diagram :graph="graph" :selected="selected" @select="id => selected = id"></diagram>
              <p v-if="!graph.nodes.length" class="dim">{{ current.type === 'local' ? (errors ? 'Le plan est refusé : voyez les diagnostics.' : 'Aucune ressource.') : 'Aucune ressource sur la plateforme pour ce projet.' }}</p>
            </div>
            <detail :node="selectedNode" :platform="current.type === 'platform' || viewPlatform" @close="selected = null" @logs="doLogs"></detail>
          </div>
          <div class="content compose" v-show="tab === 'compose'">
            <aside class="files"><a v-for="f in files" :key="f" href="#" :class="{ active: f === file.path }" @click.prevent="openFile(f)">{{ f }}</a></aside>
            <div class="editor">
              <header><code>{{ file.path }}</code><span class="grow"></span><cc-button :primary="true" :disabled="!file.dirty" title="Écrit le fichier dans le projet et recalcule le plan" @cc-click="saveFile">Enregistrer et replanifier</cc-button></header>
              <textarea spellcheck="false" :value="file.content" @input="e => { file.content = e.target.value; file.dirty = true; }"></textarea>
            </div>
          </div>
          <div class="content diagnostics" v-show="tab === 'diagnostics'">
            <p v-if="!diagnostics.length" class="dim">Aucun diagnostic : le compose se traduit sans réserve.</p>
            <cc-notice v-for="(d, i) in diagnostics" :key="i" :intent="d.severity === 'error' ? 'danger' : 'warning'" :heading="(d.file ? d.file + ':' + d.line + ':' + d.col + ' ' : '') + '[' + d.code + '] ' + (d.path || '')" :message="d.message"></cc-notice>
          </div>
          <div class="content compose" v-show="tab === 'export' && exportResult">
            <aside class="files"><a v-for="f in exportResult?.files || []" :key="f.path" href="#" :class="{ active: f.path === exportFile }" @click.prevent="exportFile = f.path">{{ f.path }}</a></aside>
            <div class="editor">
              <header><code>{{ exportFile }}</code><span class="grow"></span><label class="switch"><input type="checkbox" v-model="opts.branches"> écrire les branches git</label><cc-button :outlined="true" @cc-click="doExport">Régénérer</cc-button></header>
              <pre class="json">{{ (exportResult?.files || []).find(f => f.path === exportFile)?.content }}</pre>
              <cc-notice v-for="(d, i) in exportResult?.diagnostics || []" :key="i" intent="warning" :message="d"></cc-notice>
            </div>
          </div>
          <log-panel :lines="logs.lines" :running="logs.running" :title="logs.title" @cancel="logs.cancel && logs.cancel()" @clear="logs.lines = []"></log-panel>
        </template>
      </main>

      <cc-dialog heading="Supprimer les ressources" :open="dialog.down" @cc-close="dialog.down = false" closedBy="any">
        <p>Supprime les applications et network groups du projet <strong>{{ current?.name }}</strong>. Les addons ne sont supprimés qu'avec <code>--yes</code> (données perdues).</p>
        <label class="switch"><input type="checkbox" v-model="opts.yes"> --yes (addons compris)</label>
        <label class="switch"><input type="checkbox" v-model="opts.dry_run"> --dry-run</label>
        <div class="row-end"><cc-button :outlined="true" @cc-click="dialog.down = false">Annuler</cc-button><cc-button :danger="true" @cc-click="doDown">Down</cc-button></div>
      </cc-dialog>
      <cc-dialog heading="Charger un dump (seed)" :open="dialog.seed" @cc-close="dialog.seed = false" closedBy="any">
        <cc-select label="Service (addon)" :options="(plan?.resources || []).filter(r => r.resource === 'addon').map(r => ({ label: r.service + ' (' + r.provider + ')', value: r.service }))" :value="seed.service" placeholder="Choisir" @cc-select="e => seed.service = val(e)"></cc-select>
        <p class="hint">Formats : .sql/.sql.gz (psql) et .dump (pg_dump -Fc, via pg_restore) pour PostgreSQL, .sql/.sql.gz pour MySQL, .sh, .js/.archive (MongoDB, DocumentDB), .redis/.resp (Redis), .ndjson (Elasticsearch). Chargé une fois par contenu (témoin <code>cc_compose_init</code>).</p>
        <label class="upload" :title="'Un fichier de votre poste, copié dans le projet (.clever-compose/seed/)'"><input type="file" @change="uploadDump"><span>{{ seed.uploading ? 'Envoi…' : 'Envoyer un fichier de dump' }}</span></label>
        <cc-select label="ou un fichier déjà dans le projet" :options="files.map(f => ({ label: f, value: f }))" :value="seed.dump" placeholder="Choisir" @cc-select="e => seed.dump = val(e)"></cc-select>
        <p v-if="seed.dump"><strong>Dump :</strong> <code>{{ seed.dump }}</code></p>
        <label class="switch" title="Sinon, une Clever Task exécute le chargement sur la plateforme"><input type="checkbox" v-model="seed.local"> exécuter depuis ce poste (Docker s'il répond, sinon le client psql/mysql/mongosh/redis-cli/curl installé)</label>
        <div class="row-end"><cc-button :outlined="true" @cc-click="dialog.seed = false">Annuler</cc-button><cc-button :primary="true" :disabled="!seed.service || !seed.dump" @cc-click="doSeed">Seed</cc-button></div>
      </cc-dialog>
      <cc-dialog heading="Ouvrir un répertoire du poste" :open="dialog.addDir" @cc-close="dialog.addDir = false" closedBy="any">
        <cc-input-text label="Chemin absolu" :value="dialog.path" @cc-input="e => dialog.path = val(e)" placeholder="/home/moi/mon-projet"></cc-input-text>
        <div class="row-end"><cc-button :outlined="true" @cc-click="dialog.addDir = false">Annuler</cc-button><cc-button :primary="true" :disabled="!dialog.path" @cc-click="addDir">Ouvrir</cc-button></div>
      </cc-dialog>
    </div>
  `,
});
