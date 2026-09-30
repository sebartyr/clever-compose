<!-- L'application : projets locaux et projets sur la plateforme, diagramme,
     détail d'une ressource, éditeur du compose, diagnostics, exports,
     journal des opérations. Toutes les commandes de la CLI sont ici. -->
<script setup lang="ts">
import { computed, onMounted, reactive, ref, watch } from 'vue';
import { api, currentOrg, hasApiKey, setApiKey, setTokens, type OnLog, type Stream } from './api';
import { val } from './dom';
import { graphFromInventory, graphFromPlan } from './graph';
import Diagram from './components/Diagram.vue';
import SessionPanel from './components/SessionPanel.vue';
import LogPanel from './components/LogPanel.vue';
import NodeDetail from './components/NodeDetail.vue';
import type { AddonBody, AddonPlans, Assets, Current, Diagnostic, Drift, ExportResult, Flavors, Graph, Info, Inventory, LocalProject, Org, Plan, ScalingBody, Session } from './types';


type OptKey = 'yes' | 'dry_run' | 'allow_ephemeral' | 'no_wait' | 'assets';
const OPT_KEYS: OptKey[] = ['yes', 'dry_run', 'allow_ephemeral', 'no_wait', 'assets'];
const EMPTY_GRAPH: Graph = { nodes: [], groups: [], edges: [] };

const session = ref<Session>({ authenticated: false, org: '' });
const info = ref<Info>({});
const orgs = ref<Org[]>([]);
const locals = ref<LocalProject[]>([]);
const platformProjects = ref<string[]>([]);
const current = ref<Current | null>(null);
const tab = ref('diagram');
const plan = ref<Plan | null>(null);
const diagnostics = ref<Diagnostic[]>([]);
const inventory = ref<Inventory | null>(null);
const viewPlatform = ref(false);
const files = ref<string[]>([]);
const file = reactive({ path: '', content: '', dirty: false });
const exportResult = ref<ExportResult | null>(null);
const assets = ref<Assets | null>(null);
const assetFile = ref('');
const exportFile = ref('');
const flavors = ref<Flavors | null>(null);
const addonPlans = ref<AddonPlans | null>(null);
// Écarts entre la plateforme et le compose (réglages faits hors compose),
// repris dans x-clever sur demande ; `null` : pas encore lus.
const drifts = ref<Drift[] | null>(null);
const driftChosen = ref<string[]>([]);
const driftNote = ref('');
// Connexion : sans identifiants dans l'environnement, rien ne s'affiche
// avant la session. La page part vers Clever Cloud et revient par le
// callback du serveur, les jetons dans le fragment de l'URL.
const login = reactive({ started: false, url: '', error: '' });
// Clé d'API du serveur (`--api-key`) : demandée avant tout le reste
// quand /api/info la dit requise et qu'aucune n'est en sessionStorage.
const key = reactive({ needed: false, value: '', error: '' });
const submitKey = async () => {
  setApiKey(key.value.trim());
  key.error = '';
  try { await api.session(); key.needed = false; await boot(); } catch (e) { key.error = (e as Error).message; setApiKey(''); }
};
const startLogin = async () => {
  login.error = ''; login.started = true;
  try {
    const r = await api.login();
    login.url = r.url;
    location.href = r.url;
  } catch (e) { login.error = (e as Error).message; login.started = false; }
};
const logout = async () => { setTokens(null); await boot(); };
const selected = ref<string | null>(null);
const busy = ref(false);
const status = ref('');
const error = ref('');
const logs = reactive<{ lines: string[]; running: boolean; title: string; cancel: (() => void) | null; progress: boolean; tail: boolean }>({ lines: [], running: false, title: '', cancel: null, progress: false, tail: false });
// Une opération (up, down…) bloque les autres ; un suivi de logs, non.
const opRunning = computed(() => logs.running && !logs.tail);
// Choix de l'utilisateur gardés dans le localStorage (les jetons, eux,
// viennent de l'environnement ou de la danse OAuth) : options, requête de
// logs, seed local, dernier projet ouvert, onglet.
const remembered = <T extends object>(k: string, fallback: T): T => { try { return { ...fallback, ...(JSON.parse(localStorage.getItem('clever-compose.' + k) || 'null') || {}) }; } catch { return fallback; } };
const remember = (k: string, value: unknown) => { try { localStorage.setItem('clever-compose.' + k, JSON.stringify(value)); } catch { /* stockage indisponible */ } };
const opts = reactive(remembered('opts', { yes: false, dry_run: false, allow_ephemeral: false, no_wait: false, assets: false, branches: false, parallel: 4 }));
const seed = reactive({ service: '', dump: '', uploading: false, ...remembered('seed', { local: false }) });
const uploadDump = async (e: Event) => {
  const input = e.target as HTMLInputElement;
  const f = input.files?.[0];
  if (!f || !current.value) return;
  seed.uploading = true;
  try { const r = await api.uploadDump(current.value.id, f); seed.dump = r.path; files.value = (await api.files(current.value.id)).files; } catch (err) { notify(err); } finally { seed.uploading = false; input.value = ''; }
};
const logQuery = reactive(remembered('logs', { services: '', since: '1h', follow: true }));
watch(opts, (v) => remember('opts', v), { deep: true });
watch(() => seed.local, (local) => remember('seed', { local }));
watch(logQuery, (v) => remember('logs', v), { deep: true });
watch(current, (c) => { if (c) remember('current', { type: c.type, id: c.id, name: c.name, dir: c.dir }); });
watch(tab, (t) => remember('tab', { tab: t }));
const dialog = reactive({ down: false, seed: false, addDir: false, path: '' });

const notify = (e: unknown) => {
  const msg = (e as Error)?.message || String(e);
  error.value = msg;
  setTimeout(() => { if (error.value === msg) error.value = ''; }, 8000);
};

const loadInfo = async () => { try { info.value = await api.info(); flavors.value = await api.flavors(); addonPlans.value = await api.addonPlans(); } catch { /* serveur absent */ } };
// Scalabilité posée dans le compose (x-clever), puis fichiers et plan relus.
const setScaling = async (body: ScalingBody) => {
  if (!current.value) return;
  try {
    await api.setScaling(current.value.id, body);
    if (file.path && !file.dirty) await openFile(file.path);
    await loadPlan();
  } catch (e) { notify(e); }
};
const setAddon = async (body: AddonBody) => {
  if (!current.value) return;
  try {
    await api.setAddon(current.value.id, body);
    if (file.path && !file.dirty) await openFile(file.path);
    await loadPlan();
  } catch (e) { notify(e); }
};
// Le fichier affiché, tel qu'enregistré, téléchargé par le navigateur :
// pour remplacer le compose d'origine par la version modifiée ici.
const downloadFile = () => {
  const blob = new Blob([file.content], { type: 'text/yaml' });
  const a = document.createElement('a');
  a.href = URL.createObjectURL(blob); a.download = file.path.split('/').pop() || 'compose.yaml'; a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
};
const refreshSession = async () => {
  // Jetons rendus par le callback OAuth dans le fragment.
  if (location.hash.startsWith('#token=')) {
    const h = new URLSearchParams(location.hash.slice(1));
    setTokens({ token: h.get('token') || '', secret: h.get('secret') || '' });
    history.replaceState(null, '', location.pathname + location.search);
  }
  session.value = await api.session();
  session.value.org = currentOrg();
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

const graph = computed<Graph>(() => {
  if (current.value?.type === 'platform' || viewPlatform.value) return inventory.value ? graphFromInventory(inventory.value) : EMPTY_GRAPH;
  return plan.value ? graphFromPlan(plan.value) : EMPTY_GRAPH;
});
const selectedNode = computed(() => graph.value.nodes.find((n) => n.id === selected.value) || null);
const errors = computed(() => diagnostics.value.filter((d) => d.severity === 'error').length);
const onPlatform = computed(() => !!current.value && platformProjects.value.includes(current.value.name));

const loadPlan = async () => {
  if (current.value?.type !== 'local') return;
  busy.value = true; status.value = 'Calcul du plan (lecture des images sur leurs registres)…';
  try {
    const r = await api.plan(current.value.id, opts.allow_ephemeral);
    plan.value = r.plan; diagnostics.value = r.diagnostics;
    const errs = r.diagnostics.filter((d) => d.severity === 'error').length;
    status.value = errs ? `Plan refusé : ${errs} erreur(s), voyez les diagnostics` : `Plan à jour : ${r.plan?.resources.length ?? 0} ressources, ${r.diagnostics.length} avertissement(s)`;
    setTimeout(() => { if (status.value.startsWith('Plan')) status.value = ''; }, 6000);
  } catch (e) { status.value = ''; notify(e); } finally { busy.value = false; }
};
const loadDrifts = async () => {
  if (current.value?.type !== 'local' || !onPlatform.value || errors.value) { drifts.value = null; return; }
  try {
    drifts.value = (await api.drifts(current.value.id)).drifts;
    driftChosen.value = driftChosen.value.filter((id) => drifts.value!.some((d) => d.id === id));
  } catch (e) { drifts.value = null; notify(e); }
};
// Reprend les écarts choisis dans x-clever du compose, puis relit fichier,
// plan et écarts ; les variables reprises restent à renseigner dans .env.
const adoptDrifts = async (ids: string[]) => {
  if (!current.value || !ids.length) return;
  try {
    const r = await api.adoptDrifts(current.value.id, ids);
    const notes = [`${r.adopted.length} écart(s) repris dans ${r.path}`];
    if (r.env_to_fill.length) notes.push(`à renseigner dans .env (valeurs non copiées, le plan les exige) : ${r.env_to_fill.join(', ')}`);
    for (const k of r.skipped) notes.push(`non repris : ${k.service} : ${k.reason}`);
    driftNote.value = notes.join(' · ');
    driftChosen.value = [];
    if (file.path && !file.dirty) await openFile(file.path);
    await loadPlan();
    await loadDrifts();
  } catch (e) { notify(e); }
};
const loadInventory = async () => {
  if (!current.value || !session.value.org) return;
  try { inventory.value = await api.inventory(current.value.name); } catch (e) { inventory.value = null; if (current.value.type === 'platform') notify(e); }
};
const openLocal = async (p: LocalProject) => {
  current.value = { type: 'local', id: p.id, name: p.name, dir: p.dir };
  selected.value = null; viewPlatform.value = false; exportResult.value = null; inventory.value = null;
  tab.value = 'diagram';
  const f = await api.files(p.id);
  files.value = f.files;
  current.value.name = f.name;
  const compose = f.files.find((x) => /compose\.ya?ml$/.test(x)) || f.files[0];
  if (compose) await openFile(compose);
  await loadPlan();
  await loadInventory();
  drifts.value = null; driftNote.value = '';
  await loadDrifts();
};
const loadAssets = async () => {
  assets.value = null;
  if (!current.value || !session.value.org) return;
  try {
    const a = await api.assets(current.value.name);
    assets.value = a.present ? a : null;
    if (assets.value) assetFile.value = assets.value.files?.find((f) => /compose\.ya?ml$/.test(f.path))?.path || assets.value.files?.[0]?.path || '';
  } catch { assets.value = null; }
};
const openPlatform = async (name: string) => {
  current.value = { type: 'platform', id: name, name };
  selected.value = null; plan.value = null; diagnostics.value = []; exportResult.value = null;
  tab.value = 'diagram';
  await loadInventory();
  await loadAssets();
  if (assets.value?.plan) plan.value = assets.value.plan;
};
const openFromAssets = async () => {
  if (!current.value) return;
  try {
    const r = await api.assetsOpen(current.value.name);
    await refreshLocals();
    const p = locals.value.find((x) => x.id === r.id);
    if (p) await openLocal(p);
  } catch (e) { notify(e); }
};
const openFile = async (path: string) => {
  if (!current.value) return;
  const r = await api.readFile(current.value.id, path);
  file.path = r.path; file.content = r.content; file.dirty = false;
};
const saveFile = async () => {
  if (!current.value) return;
  await api.writeFile(current.value.id, file.path, file.content);
  file.dirty = false;
  await loadPlan();
};

const run = (title: string, starter: (onLog: OnLog) => Stream, tail = false) => {
  if (logs.running && logs.cancel) logs.cancel();
  logs.lines = []; logs.running = true; logs.title = title; logs.tail = tail;
  // Une ligne de progression (préfixe , temps écoulé) remplace la
  // précédente au lieu de s'empiler.
  const ctl = starter((line) => {
    if (line.startsWith('')) {
      const text = line.slice(1);
      if (logs.progress && logs.lines.length) logs.lines[logs.lines.length - 1] = text; else logs.lines.push(text);
      logs.progress = true;
    } else { logs.lines.push(line); logs.progress = false; }
  });
  logs.cancel = ctl.cancel;
  ctl.done.then(() => { logs.lines.push(tail ? '— fin du flux —' : '✔ terminé'); }).catch((e: Error) => { if (e.name !== 'AbortError') logs.lines.push('erreur : ' + e.message); }).finally(async () => {
    logs.running = false; logs.cancel = null;
    if (tail) return;
    await refreshPlatform();
    await loadInventory();
    if (current.value?.type === 'local') viewPlatform.value = !!inventory.value;
    await loadDrifts();
  });
};
const doUp = () => { const c = current.value!; run('up ' + c.name, (onLog) => api.up(c.id, { yes: opts.yes, dry_run: opts.dry_run, allow_ephemeral: opts.allow_ephemeral, no_wait: opts.no_wait, assets: opts.assets, parallel: Math.max(1, Math.floor(Number(opts.parallel)) || 4) }, onLog)); };
const doStop = () => { const c = current.value!; run('stop ' + c.name, (onLog) => api.stop(c.name, onLog)); };
const doStart = () => { const c = current.value!; run('start ' + c.name, (onLog) => api.start(c.name, onLog)); };
const doDown = () => { const c = current.value!; dialog.down = false; run('down ' + c.name, (onLog) => api.down(c.name, { yes: opts.yes, dry_run: opts.dry_run }, onLog)); };
// Exécution depuis le poste (`seed --local`) : permise par le serveur
// (`local_exec`) et client servi par ses identifiants ; ailleurs le
// serveur la refuse (403).
const localExecution = computed(() => info.value.local_exec !== false && !session.value?.own_tokens);
const doSeed = () => { const c = current.value!; dialog.seed = false; run('seed ' + seed.service, (onLog) => api.seed(c.id, { service: seed.service, dump: seed.dump, local: seed.local && localExecution.value, yes: opts.yes, no_wait: opts.no_wait }, onLog)); };
const doLogs = (service?: string) => {
  const c = current.value!;
  const services = service ? [service] : logQuery.services.split(',').map((s) => s.trim()).filter(Boolean);
  run('logs ' + c.name + (services.length ? ' ' + services.join(' ') : ''), (onLog) => api.logs(c.name, { services, since: logQuery.since, follow: logQuery.follow }, onLog), true);
};
const doAssetsSync = () => { const c = current.value!; run('assets ' + c.name, (onLog) => api.syncAssets(c.id, onLog)); };
const doExport = async () => {
  if (!current.value) return;
  busy.value = true;
  try {
    exportResult.value = await api.exportProject(current.value.id, { branches: opts.branches, allow_ephemeral: opts.allow_ephemeral });
    exportFile.value = exportResult.value.files[0]?.path || '';
    tab.value = 'export';
  } catch (e) { notify(e); } finally { busy.value = false; }
};
const upload = async (e: Event) => {
  const input = e.target as HTMLInputElement;
  const f = input.files?.[0];
  if (!f) return;
  try { const r = await api.upload(f); await refreshLocals(); const p = locals.value.find((x) => x.id === r.id); if (p) await openLocal(p); } catch (err) { notify(err); }
  input.value = '';
};
const addDir = async () => {
  try { const r = await api.addDir(dialog.path); dialog.addDir = false; dialog.path = ''; await refreshLocals(); const p = locals.value.find((x) => x.id === r.id); if (p) await openLocal(p); } catch (e) { notify(e); }
};
const removeLocal = async (p: LocalProject) => { await api.removeLocal(p.id); if (current.value?.id === p.id) current.value = null; await refreshLocals(); };
const setOptions = (e: Event) => { const v: string[] = val(e) || []; for (const k of OPT_KEYS) opts[k] = v.includes(k); };
const editFile = (e: Event) => { file.content = (e.target as HTMLTextAreaElement).value; file.dirty = true; };

watch(() => opts.allow_ephemeral, loadPlan);
// Ce qui s'affiche une fois la session ouverte.
const boot = async () => {
  if (info.value.api_key && !hasApiKey()) { key.needed = true; return; }
  try { await refreshSession(); } catch (e) {
    const msg = (e as Error).message;
    if (/401|clé/.test(msg)) { setApiKey(''); key.needed = true; key.error = msg; return; }
    throw e;
  }
  if (!session.value.authenticated) {
    // Jetons refusés (expirés, révoqués) : on les oublie et on propose
    // la connexion, en gardant la raison affichée.
    if (session.value.own_tokens) setTokens(null);
    await startLogin();
    return;
  }
  await refreshLocals();
  // Liens directs : ?project=<nom> (plateforme) ou ?local=<id>.
  const q = new URLSearchParams(location.search);
  const last = remembered<Partial<Current>>('current', {});
  const wantedLocal = q.get('local');
  if (q.get('project') && session.value.org) await openPlatform(q.get('project')!);
  else if (wantedLocal && locals.value.some((p) => p.id === wantedLocal)) await openLocal(locals.value.find((p) => p.id === wantedLocal)!);
  else if (last.type === 'local' && last.dir && locals.value.some((p) => p.dir === last.dir)) await openLocal(locals.value.find((p) => p.dir === last.dir)!);
  else if (last.type === 'local' && last.dir) { try { const r = await api.addDir(last.dir); await refreshLocals(); const p = locals.value.find((x) => x.id === r.id); if (p) await openLocal(p); } catch { /* répertoire disparu */ } }
  else if (last.type === 'platform' && last.name && session.value.org && platformProjects.value.includes(last.name)) await openPlatform(last.name);
  else if (locals.value.length) await openLocal(locals.value[0]);
  const t = remembered<{ tab?: string }>('tab', {}).tab;
  if (t && current.value && ['diagram', 'compose', 'diagnostics'].includes(t)) tab.value = t;
};
onMounted(async () => { await loadInfo(); await boot(); });
// Menu latéral replié sur petit écran.
const sideOpen = ref(false);
watch(current, () => { sideOpen.value = false; });
</script>

<template>
  <div v-if="key.needed" class="login">
    <cc-block>
      <div slot="header-title">Clé d'API du serveur</div>
      <div slot="content" class="login-body">
        <p>Ce serveur exige une clé (<code>--api-key</code>). Elle reste dans cet onglet.</p>
        <cc-notice v-if="key.error" intent="danger" :message="key.error"></cc-notice>
        <cc-input-text label="Clé" :secret="true" :value="key.value" @cc-input="(e: CustomEvent) => key.value = e.detail" @cc-requestimplicitsubmit="submitKey"></cc-input-text>
        <p><cc-button :primary="true" :disabled="!key.value.trim()" title="Vérifie la clé auprès du serveur" @cc-click="submitKey">Continuer</cc-button></p>
      </div>
    </cc-block>
  </div>
  <div v-else-if="!session.authenticated" class="login">
    <cc-block>
      <div slot="header-title">Connexion à Clever Cloud</div>
      <div slot="content" class="login-body">
        <p v-if="session.own_tokens && session.reason" class="dim">Jetons précédents refusés : {{ session.reason }}</p>
        <p v-if="login.started && !login.error">Redirection vers Clever Cloud…</p>
        <p v-else>{{ session.own_tokens ? 'Vos jetons ne sont plus acceptés (' + session.reason + ') :' : 'Ce serveur n’a pas d’identifiants :' }} autorisez l'accès depuis votre compte Clever Cloud. Les jetons restent dans cet onglet et le serveur agit en votre nom.</p>
        <cc-notice v-if="login.error" intent="danger" :message="login.error"></cc-notice>
        <p><cc-button :primary="true" :waiting="login.started && !login.error" title="Autorisation via Clever Cloud" @cc-click="startLogin">Se connecter</cc-button></p>
      </div>
    </cc-block>
  </div>
  <div v-else class="layout" :class="{ 'side-open': sideOpen }">
    <header class="top">
      <cc-button class="menu" :outlined="true" :hideText="true" title="Projets" @cc-click="sideOpen = !sideOpen">Projets</cc-button>
      <h1>clever-compose <small class="dim" :title="'build ' + (info.built || '')">{{ info.version }}{{ info.commit ? ' · ' + info.commit : '' }}</small></h1>
      <span class="grow"></span>
      <SessionPanel :session="session" :orgs="orgs" @changed="refreshSession" @login="startLogin" @logout="logout" />
    </header>
    <nav class="side">
      <cc-button class="close-side" :outlined="true" title="Ferme le menu des projets" @cc-click="sideOpen = false">Fermer</cc-button>
      <h2>Projets locaux</h2>
      <ul>
        <li v-for="p in locals" :key="p.id" :class="{ active: current?.id === p.id }">
          <a href="#" @click.prevent="openLocal(p)">{{ p.name }}</a>
          <span class="dim" :title="p.dir">{{ p.temporary ? 'archive' : p.dir }}</span>
          <cc-button :outlined="true" :hideText="true" :danger="true" title="Retire le projet de la liste (une archive extraite est supprimée du disque)" @cc-click="removeLocal(p)">Retirer</cc-button>
        </li>
      </ul>
      <label class="upload" title="Archive .zip, .tar.gz, .tgz ou .tar contenant le projet ; extraite dans un répertoire temporaire"><input type="file" accept=".zip,.tar,.tgz,.tar.gz" @change="upload"><span>Charger une archive (.zip, .tar.gz)</span></label>
      <cc-button v-if="!session.own_tokens" :outlined="true" title="Un répertoire du poste qui contient un compose.yaml (usage local seulement)" @cc-click="dialog.addDir = true">Ouvrir un répertoire du poste</cc-button>
      <h2>Sur la plateforme <cc-button :outlined="true" :hideText="true" title="Relit les projets de l'organisation" @cc-click="refreshPlatform">Rafraîchir</cc-button></h2>
      <p v-if="!session.authenticated" class="dim">Connectez-vous (en haut à droite) pour voir les projets déployés.</p>
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
            <cc-toggle legend="Options" :hideText="false" :inline="true" title="--yes : confirme la création des addons (facturés). --dry-run : décrit sans rien modifier. --allow-ephemeral : volumes nommés acceptés comme stockage éphémère. --no-wait : n'attend pas les déploiements. --assets : fichiers et plan conservés dans le bucket Cellar configuré (CLEVER_COMPOSE_ASSETS_BUCKET)." :choices="[{label:'--yes', value:'yes'},{label:'--dry-run', value:'dry_run'},{label:'--allow-ephemeral', value:'allow_ephemeral'},{label:'--no-wait', value:'no_wait'},{label:'--assets', value:'assets'}]" :multipleValues="OPT_KEYS.filter(k => opts[k])" @cc-multi-select="setOptions"></cc-toggle>
            <cc-input-text class="parallel" label="Parallèle" type="number" :inline="true" :value="String(opts.parallel)" @cc-input="(e: CustomEvent) => opts.parallel = e.detail" title="--parallel : déploiements menés en même temps, chacun dès que ses dépendances sont déployées (1 : l'un après l'autre)"></cc-input-text>
            <cc-button :outlined="true" :waiting="busy" title="Recalcule le plan et les diagnostics, sans rien créer" @cc-click="loadPlan">Plan</cc-button>
            <cc-button :primary="true" :disabled="errors > 0 || opRunning" title="Crée ou met à jour les ressources et déploie (--yes confirme les addons, facturés)" @cc-click="doUp">Up</cc-button>
            <cc-button :outlined="true" :disabled="!plan || opRunning" title="Charge un dump local dans un addon, une fois par contenu" @cc-click="dialog.seed = true">Seed</cc-button>
            <cc-button :outlined="true" :disabled="errors > 0" title="Terraform (JSON et HCL) et script clever-tools" @cc-click="doExport">Export</cc-button>
          </template>
          <cc-button v-if="onPlatform || current.type === 'platform'" :outlined="true" :disabled="opRunning" title="Logs des applications et addons du projet" @cc-click="doLogs()">Logs</cc-button>
          <cc-button v-if="onPlatform || current.type === 'platform'" :outlined="true" :disabled="opRunning" title="Arrête les applications sans rien supprimer (comme docker compose stop)" @cc-click="doStop">Stop</cc-button>
          <cc-button v-if="onPlatform || current.type === 'platform'" :outlined="true" :disabled="opRunning" title="Relance les applications arrêtées" @cc-click="doStart">Start</cc-button>
          <cc-button v-if="onPlatform || current.type === 'platform'" :danger="true" :outlined="true" :disabled="opRunning" title="Supprime les applications et network groups ; les addons seulement avec --yes" @cc-click="dialog.down = true">Down</cc-button>
        </section>
        <p v-if="status" class="status"><cc-loader v-if="busy" a11y-name="en cours"></cc-loader> {{ status }}</p>
        <nav class="tabs">
          <a :class="{ active: tab === 'diagram' }" href="#" @click.prevent="tab = 'diagram'">Diagramme</a>
          <a v-if="current.type === 'local'" :class="{ active: tab === 'compose' }" href="#" @click.prevent="tab = 'compose'">Compose</a>
          <a v-if="current.type === 'platform' && assets" :class="{ active: tab === 'assets' }" href="#" @click.prevent="tab = 'assets'">Fichiers (Cellar)</a>
          <a v-if="current.type === 'local'" :class="{ active: tab === 'diagnostics' }" href="#" @click.prevent="tab = 'diagnostics'">Diagnostics <cc-badge v-if="diagnostics.length" :intent="errors ? 'danger' : 'warning'" weight="dimmed" circle>{{ diagnostics.length }}</cc-badge></a>
          <a v-if="current.type === 'local' && onPlatform" :class="{ active: tab === 'drift' }" href="#" @click.prevent="tab = 'drift'; loadDrifts()">Écarts <cc-badge v-if="drifts?.length" intent="warning" weight="dimmed" circle>{{ drifts.length }}</cc-badge></a>
          <a v-if="exportResult" :class="{ active: tab === 'export' }" href="#" @click.prevent="tab = 'export'">Export</a>
          <span class="grow"></span>
          <label v-if="current.type === 'local' && onPlatform" class="switch" title="Diagramme de ce qui existe sur la plateforme (instances, déploiements) au lieu du plan"><input type="checkbox" v-model="viewPlatform" @change="loadInventory"> état sur la plateforme</label>
          <cc-button v-if="current.type === 'platform' && assets" :outlined="true" title="Rapatrie les fichiers du bucket Cellar dans un projet local temporaire : plan, édition, up…" @cc-click="openFromAssets">Ouvrir en local</cc-button>
          <cc-button v-if="current.type === 'platform'" :outlined="true" title="Relit l'état des ressources sur la plateforme" @cc-click="loadInventory">Rafraîchir</cc-button>
        </nav>
        <div class="content" v-show="tab === 'diagram'">
          <div class="canvas">
            <Diagram :graph="graph" :selected="selected" @select="(id) => selected = id" />
            <p v-if="!graph.nodes.length" class="dim">{{ current.type === 'local' ? (errors ? 'Le plan est refusé : voyez les diagnostics.' : 'Aucune ressource.') : 'Aucune ressource sur la plateforme pour ce projet.' }}</p>
            <p v-if="current.type === 'platform' && assets" class="dim">Assets conservés dans Cellar (bucket {{ assets.bucket }}, {{ assets.manifest?.files.length }} fichiers, {{ assets.manifest?.uploaded_at }}) : onglet « Fichiers (Cellar) », ou « Ouvrir en local ».</p>
            <p v-else-if="current.type === 'platform'" class="dim">Pas d'assets dans Cellar pour ce projet : relancez <code>up</code> avec <code>--assets</code> ou <code>x-clever.assets: true</code> pour que le compose et le plan soient lisibles ici.</p>
          </div>
          <NodeDetail :node="selectedNode" :platform="current.type === 'platform' || viewPlatform" :flavors="flavors" :addonPlans="addonPlans" @close="selected = null" @logs="doLogs" @scaling="setScaling" @addon="setAddon" />
        </div>
        <div class="content drift" v-show="tab === 'drift'">
          <p class="dim">Réglages faits sur la plateforme hors du compose (console, clever-tools). <code>up</code> écrase le scaling et le build dédié, garde les variables ajoutées sans les montrer, ne migre pas un addon existant. Les reprendre écrit <code>x-clever</code> du compose ; une variable est reprise sans sa valeur (<code>${NOM:?…}</code>, à renseigner dans <code>.env</code>).</p>
          <cc-notice v-if="driftNote" intent="info" :message="driftNote" :closeable="true" @cc-notice-dismiss="driftNote = ''"></cc-notice>
          <p v-if="drifts === null" class="dim">{{ errors ? 'Plan refusé : les écarts se comparent au plan.' : 'Écarts non lus.' }}</p>
          <p v-else-if="!drifts.length" class="dim">Aucun écart : la plateforme suit le compose.</p>
          <template v-else>
            <label v-for="d in drifts" :key="d.id" class="drift-item"><input type="checkbox" :value="d.id" v-model="driftChosen"> {{ d.text }}</label>
            <div class="drift-actions">
              <cc-button :primary="true" :disabled="!driftChosen.length" title="Écrit les écarts cochés dans x-clever du compose et recalcule le plan" @cc-click="adoptDrifts(driftChosen)">Reprendre la sélection dans le compose</cc-button>
              <cc-button :outlined="true" title="Écrit tous les écarts dans x-clever du compose" @cc-click="adoptDrifts(drifts.map((d) => d.id))">Tout reprendre</cc-button>
              <cc-button :outlined="true" title="Relit la plateforme" @cc-click="loadDrifts">Rafraîchir</cc-button>
            </div>
          </template>
        </div>
        <div class="content compose" v-show="tab === 'compose'">
          <aside class="files"><a v-for="f in files" :key="f" href="#" :class="{ active: f === file.path }" @click.prevent="openFile(f)">{{ f }}</a></aside>
          <div class="editor">
            <header>
              <code>{{ file.path }}</code><span class="grow"></span>
              <cc-button :outlined="true" :disabled="!file.path" title="Télécharge ce fichier tel qu'affiché, pour remplacer celui de votre dépôt" @cc-click="downloadFile">Télécharger</cc-button>
              <cc-button v-if="info.assets_bucket" :outlined="true" :disabled="opRunning" title="Pousse les fichiers du projet (compose, .env, build…) dans le bucket Cellar, sans déployer" @cc-click="doAssetsSync">Mettre à jour les assets</cc-button>
              <cc-button :primary="true" :disabled="!file.dirty" title="Écrit le fichier dans le projet et recalcule le plan" @cc-click="saveFile">Enregistrer et replanifier</cc-button>
            </header>
            <textarea spellcheck="false" :value="file.content" @input="editFile"></textarea>
          </div>
        </div>
        <div class="content diagnostics" v-show="tab === 'diagnostics'">
          <p v-if="!diagnostics.length" class="dim">Aucun diagnostic : le compose se traduit sans réserve.</p>
          <cc-notice v-for="(d, i) in diagnostics" :key="i" :intent="d.severity === 'error' ? 'danger' : 'warning'" :heading="(d.file ? d.file + ':' + d.line + ':' + d.col + ' ' : '') + '[' + d.code + '] ' + (d.path || '')" :message="d.message"></cc-notice>
        </div>
        <div class="content compose" v-show="tab === 'assets' && assets">
          <aside class="files"><a v-for="f in assets?.files || []" :key="f.path" href="#" :class="{ active: f.path === assetFile }" @click.prevent="assetFile = f.path">{{ f.path }}</a></aside>
          <div class="editor">
            <header><code>{{ assetFile }}</code><span class="grow"></span><span class="dim">bucket {{ assets?.bucket }} · {{ assets?.manifest?.uploaded_at }}</span></header>
            <pre class="json">{{ (assets?.files || []).find(f => f.path === assetFile)?.content ?? '(binaire)' }}</pre>
          </div>
        </div>
        <div class="content compose" v-show="tab === 'export' && exportResult">
          <aside class="files"><a v-for="f in exportResult?.files || []" :key="f.path" href="#" :class="{ active: f.path === exportFile }" @click.prevent="exportFile = f.path">{{ f.path }}</a></aside>
          <div class="editor">
            <header><code>{{ exportFile }}</code><span class="grow"></span><label class="switch"><input type="checkbox" v-model="opts.branches"> écrire les branches git</label><cc-button :outlined="true" @cc-click="doExport">Régénérer</cc-button></header>
            <pre class="json">{{ (exportResult?.files || []).find(f => f.path === exportFile)?.content }}</pre>
            <cc-notice v-for="(d, i) in exportResult?.diagnostics || []" :key="i" intent="warning" :message="d"></cc-notice>
          </div>
        </div>
        <LogPanel :lines="logs.lines" :running="logs.running" :title="logs.title" @cancel="logs.cancel && logs.cancel()" @clear="logs.lines = []" />
      </template>
    </main>

    <cc-dialog heading="Supprimer les ressources" :open="dialog.down" @cc-close="dialog.down = false" closedBy="any">
      <p>Supprime les applications et network groups du projet <strong>{{ current?.name }}</strong>. Les addons ne sont supprimés qu'avec <code>--yes</code> (données perdues).</p>
      <label class="switch"><input type="checkbox" v-model="opts.yes"> --yes (addons compris)</label>
      <label class="switch"><input type="checkbox" v-model="opts.dry_run"> --dry-run</label>
      <div class="row-end"><cc-button :outlined="true" @cc-click="dialog.down = false">Annuler</cc-button><cc-button :danger="true" @cc-click="doDown">Down</cc-button></div>
    </cc-dialog>
    <cc-dialog heading="Charger un dump (seed)" :open="dialog.seed" @cc-close="dialog.seed = false" closedBy="any">
      <cc-select label="Service (addon)" :options="(plan?.resources || []).filter(r => r.resource === 'addon').map(r => ({ label: r.service + ' (' + r.provider + ')', value: r.service }))" :value="seed.service" placeholder="Choisir" @cc-select="(e: Event) => seed.service = val(e)"></cc-select>
      <p class="hint">Formats : .sql/.sql.gz (psql) et .dump (pg_dump -Fc, via pg_restore) pour PostgreSQL, .sql/.sql.gz pour MySQL, .sh, .js/.archive (MongoDB, DocumentDB), .redis/.resp (Redis), .ndjson (Elasticsearch). Chargé une fois par contenu (témoin <code>cc_compose_init</code>).</p>
      <label class="upload" :title="'Un fichier de votre poste, copié dans le projet (.clever-compose/seed/)'"><input type="file" @change="uploadDump"><span>{{ seed.uploading ? 'Envoi…' : 'Envoyer un fichier de dump' }}</span></label>
      <cc-select label="ou un fichier déjà dans le projet" :options="files.map(f => ({ label: f, value: f }))" :value="seed.dump" placeholder="Choisir" @cc-select="(e: Event) => seed.dump = val(e)"></cc-select>
      <p v-if="seed.dump"><strong>Dump :</strong> <code>{{ seed.dump }}</code></p>
      <label v-if="localExecution" class="switch" title="Sinon, une Clever Task exécute le chargement sur la plateforme"><input type="checkbox" v-model="seed.local"> exécuter depuis ce poste (Docker s'il répond, sinon le client psql/mysql/mongosh/redis-cli/curl installé)</label>
      <p v-else class="dim">Une Clever Task exécute le chargement sur la plateforme (l'exécution depuis le poste est réservée à un serveur local).</p>
      <div class="row-end"><cc-button :outlined="true" @cc-click="dialog.seed = false">Annuler</cc-button><cc-button :primary="true" :disabled="!seed.service || !seed.dump" @cc-click="doSeed">Seed</cc-button></div>
    </cc-dialog>
    <cc-dialog heading="Ouvrir un répertoire du poste" :open="dialog.addDir" @cc-close="dialog.addDir = false" closedBy="any">
      <cc-input-text label="Chemin absolu" :value="dialog.path" @cc-input="(e: Event) => dialog.path = val(e)" placeholder="/home/moi/mon-projet"></cc-input-text>
      <div class="row-end"><cc-button :outlined="true" @cc-click="dialog.addDir = false">Annuler</cc-button><cc-button :primary="true" :disabled="!dialog.path" @cc-click="addDir">Ouvrir</cc-button></div>
    </cc-dialog>
  </div>
</template>
