<!-- Le diagramme des ressources : SVG, une boîte par network group, un
     nœud par application, tâche ou addon, des liens pour les dépendances.
     Le nœud cliqué est signalé au parent (`select`). -->
<script setup lang="ts">
import { computed } from 'vue';
import { layout } from '../graph';
import type { Graph, GraphNode, NodeKind, NodeStatus } from '../types';

const props = defineProps<{ graph: Graph; selected: string | null }>();
const emit = defineEmits<{ select: [id: string] }>();

const KIND_LABEL: Record<NodeKind, string> = { app: 'application', task: 'tâche', addon: 'addon' };
const STATUS: Record<NodeStatus, { label: string; color: string }> = {
  planned: { label: 'à créer', color: 'var(--cc-color-text-weak)' },
  up: { label: 'en ligne', color: 'var(--cc-color-text-success)' },
  deploying: { label: 'déploiement', color: 'var(--cc-color-text-warning)' },
  failed: { label: 'en échec', color: 'var(--cc-color-text-danger)' },
  stopped: { label: 'arrêtée', color: 'var(--cc-color-text-weak)' },
  done: { label: 'terminée', color: 'var(--cc-color-text-success)' },
};

const lay = computed(() => layout(props.graph));
const nodes = computed(() => props.graph.nodes.map((n) => ({ ...n, pos: lay.value.positions.get(n.id)! })).filter((n) => n.pos));
const edges = computed(() => props.graph.edges.flatMap((e) => {
  const a = lay.value.positions.get(e.from);
  const b = lay.value.positions.get(e.to);
  if (!a || !b) return [];
  const { W, H } = lay.value;
  const x1 = a.x + W; const y1 = a.y + H / 2;
  const x2 = b.x; const y2 = b.y + H / 2;
  const c = Math.max(30, (x2 - x1) / 2);
  return [{ ...e, d: `M ${x1} ${y1} C ${x1 + c} ${y1}, ${x2 - c} ${y2}, ${x2} ${y2}` }];
}));
const statusOf = (n: GraphNode) => STATUS[n.status] || STATUS.planned;
const select = (n: GraphNode) => emit('select', n.id);
</script>

<template>
  <div class="diagram" :style="{ minHeight: '200px' }">
    <svg :viewBox="'0 0 ' + lay.width + ' ' + lay.height" :width="lay.width" :height="lay.height" role="img" aria-label="Diagramme des ressources">
      <defs>
        <marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="8" markerHeight="8" orient="auto-start-reverse">
          <path d="M 0 0 L 10 5 L 0 10 z" fill="var(--cc-color-text-weak)" />
        </marker>
      </defs>
      <g v-for="b in lay.boxes" :key="b.id" :class="['box', b.kind]">
        <rect :x="b.x" :y="b.y" :width="b.width" :height="b.height" rx="10" />
        <text :x="b.x + 14" :y="b.y + 19" class="box-title">{{ b.kind === 'group' ? 'network group · ' + b.label : b.label }}</text>
      </g>
      <path v-for="(e, i) in edges" :key="'e' + i" :d="e.d" :class="['edge', e.kind]" marker-end="url(#arrow)" />
      <g v-for="n in nodes" :key="n.id" :class="['node', n.kind, { selected: n.id === selected }]" :transform="'translate(' + n.pos.x + ',' + n.pos.y + ')'" @click="select(n)" tabindex="0" @keydown.enter="select(n)" role="button" :aria-label="KIND_LABEL[n.kind] + ' ' + n.label">
        <rect :width="lay.W" :height="lay.H" rx="8" />
        <text x="12" y="22" class="node-title">{{ n.label }}</text>
        <text x="12" y="41" class="node-sub">{{ n.sub.length > 34 ? n.sub.slice(0, 33) + '…' : n.sub }}</text>
        <text x="12" y="57" class="node-kind">{{ KIND_LABEL[n.kind] }}</text>
        <circle :cx="lay.W - 14" cy="14" r="5" :fill="statusOf(n).color"><title>{{ statusOf(n).label }}</title></circle>
      </g>
    </svg>
  </div>
</template>
