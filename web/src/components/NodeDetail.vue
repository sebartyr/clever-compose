<!-- Détail du nœud sélectionné dans le diagramme : ce que le plan en dit
     (projet local) ou ce que la plateforme en montre (inventaire), le
     formulaire de scalabilité et le JSON brut. -->
<script setup lang="ts">
import { computed, reactive, watch } from 'vue';
import type { Flavors, GraphNode, ScalingBody } from '../types';

const props = defineProps<{ node: GraphNode | null; platform: boolean; flavors: Flavors | null }>();
const emit = defineEmits<{ close: []; logs: [service: string]; scaling: [body: ScalingBody] }>();

const env = computed(() => ((props.node?.data?.env || []) as { key: string; value: any[] }[]).map((e) => ({ key: e.key, value: e.value.map((s) => (s.literal !== undefined ? s.literal : `\${${s.ref.var || s.ref.member || s.ref.name}}`)).join('') })));
const json = computed(() => JSON.stringify(props.node?.data || {}, null, 2));
// Formulaire de scalabilité, rempli depuis le plan à chaque sélection ;
// « Appliquer » l'écrit dans x-clever du compose.
const scaling = reactive({ min_flavor: '', max_flavor: '', min_instances: 1 as number | string, max_instances: 1 as number | string, saving: false });
watch(() => props.node, (n) => {
  const s = n?.data?.scaling;
  if (!s) return;
  scaling.min_flavor = s.min_flavor; scaling.max_flavor = s.max_flavor; scaling.min_instances = s.min_instances; scaling.max_instances = s.max_instances;
}, { immediate: true });
const flavorOptions = computed(() => (props.flavors?.flavors || []).map((f) => ({ value: f.name, label: `${f.name} — ${f.cpus} vCPU, ${Math.round(f.memory_bytes / 1048576)} Mio` })));
const scalingDirty = computed(() => {
  const s = props.node?.data?.scaling;
  return !!s && (s.min_flavor !== scaling.min_flavor || s.max_flavor !== scaling.max_flavor || s.min_instances !== Number(scaling.min_instances) || s.max_instances !== Number(scaling.max_instances));
});
const applyScaling = () => {
  if (!props.node) return;
  emit('scaling', { service: props.node.data.service, min_flavor: scaling.min_flavor, max_flavor: scaling.max_flavor, min_instances: Number(scaling.min_instances), max_instances: Number(scaling.max_instances) });
};
</script>

<template>
  <aside class="detail" v-if="node">
    <header>
      <cc-badge :weight="'dimmed'" :intent="node.kind === 'addon' ? 'info' : 'neutral'">{{ node.kind }}</cc-badge>
      <strong>{{ node.label }}</strong>
      <span class="grow"></span>
      <cc-button :outlined="true" :hideText="true" title="Fermer le détail" @cc-click="emit('close')">Fermer</cc-button>
    </header>
    <p class="sub">{{ node.sub }}</p>
    <template v-if="platform && node.kind !== 'addon'">
      <dl>
        <dt>Identifiant</dt><dd><code>{{ node.data.id }}</code></dd>
        <dt>Instances</dt><dd>{{ node.data.instances.length ? node.data.instances.map((i: any) => i.state.toLowerCase() + ' (' + (i.flavor || '?') + ')').join(', ') : 'aucune' }}</dd>
        <dt>Dernier déploiement</dt><dd>{{ node.data.last_deployment ? node.data.last_deployment.state : 'aucun' }}</dd>
        <dt v-if="node.data.domains.length">Domaines</dt><dd v-if="node.data.domains.length"><a v-for="d in node.data.domains" :key="d" :href="'https://' + d" target="_blank" rel="noopener">{{ d }}</a></dd>
        <dt>Tags</dt><dd>{{ node.data.tags.join(', ') || '—' }}</dd>
      </dl>
      <cc-button :outlined="true" @cc-click="emit('logs', node.label)">Logs de {{ node.label }}</cc-button>
    </template>
    <template v-else-if="platform">
      <dl>
        <dt>Identifiant</dt><dd><code>{{ node.data.id }}</code></dd>
        <dt>Id réel</dt><dd><code>{{ node.data.real_id }}</code></dd>
        <dt>Fournisseur</dt><dd>{{ node.data.provider }} · plan {{ node.data.plan }}</dd>
      </dl>
      <cc-button :outlined="true" @cc-click="emit('logs', node.label)">Logs de {{ node.label }}</cc-button>
    </template>
    <template v-else-if="node.kind !== 'addon'">
      <dl>
        <dt>Nom sur la plateforme</dt><dd><code>{{ node.data.name }}</code></dd>
        <dt>Région</dt><dd>{{ node.data.region }}</dd>
        <dt>Commande</dt><dd><code>{{ node.data.shim.argv.join(' ') }}</code></dd>
        <dt>Port</dt><dd>{{ node.data.shim.port.mode }}{{ node.data.shim.port.target_port ? ' → ' + node.data.shim.port.target_port : '' }}</dd>
        <dt v-if="node.data.depends_on.length">Dépend de</dt><dd v-if="node.data.depends_on.length">{{ node.data.depends_on.join(', ') }}</dd>
      </dl>
      <h4>Scalabilité</h4>
      <div class="scaling" v-if="node.data.scaling">
        <cc-select label="Flavor minimale" :options="flavorOptions" :value="scaling.min_flavor" @cc-select="(e: CustomEvent) => scaling.min_flavor = e.detail" title="Taille d'instance au repos (x-clever.min_flavor, ou flavor si égale à la maximale)"></cc-select>
        <cc-select label="Flavor maximale" :options="flavorOptions" :value="scaling.max_flavor" @cc-select="(e: CustomEvent) => scaling.max_flavor = e.detail" title="Taille d'instance sous charge (x-clever.max_flavor)"></cc-select>
        <cc-input-text label="Instances minimales" type="number" :value="String(scaling.min_instances)" @cc-input="(e: CustomEvent) => scaling.min_instances = e.detail" title="x-clever.min_instances"></cc-input-text>
        <cc-input-text label="Instances maximales" type="number" :value="String(scaling.max_instances)" @cc-input="(e: CustomEvent) => scaling.max_instances = e.detail" :title="'x-clever.max_instances (au plus ' + (flavors?.max_instances || '?') + ')'"></cc-input-text>
        <cc-button :primary="true" :disabled="!scalingDirty" :waiting="scaling.saving" title="Écrit ces valeurs dans x-clever du compose et recalcule le plan ; un up appliquera le changement aux applications existantes sans les recréer" @cc-click="applyScaling">Appliquer au compose</cc-button>
      </div>
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
</template>
