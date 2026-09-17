// Le modèle du diagramme, construit soit depuis un plan (projet local :
// ce qui serait créé), soit depuis l'inventaire de la plateforme (ce qui
// existe, avec l'état des instances). Même forme dans les deux cas :
//   nodes  : { id, kind: app|task|addon, label, sub, status, data }
//   groups : { id, label, members: [nodeId] }   (network groups)
//   edges  : { from, to, kind: depends|link }
import type { Graph, GraphEdge, GraphGroup, GraphNode, Inventory, NodeStatus, Plan } from './types';

const providerLabel = (p: string): string => ({ 'postgresql-addon': 'PostgreSQL', 'mysql-addon': 'MySQL', 'redis-addon': 'Redis', 'mongodb-addon': 'MongoDB', 'es-addon': 'Elasticsearch', 'config-provider': 'Config' } as Record<string, string>)[p] || p;

export function graphFromPlan(plan: Plan): Graph {
  const nodes: GraphNode[] = [];
  const groups: GraphGroup[] = [];
  const edges: GraphEdge[] = [];
  for (const r of plan.resources) {
    if (r.resource === 'app') {
      const image = r.source.type === 'synthetic' ? r.source.image : `build ${r.source.context}`;
      const scale = r.scaling.min_flavor === r.scaling.max_flavor && r.scaling.min_instances === r.scaling.max_instances
        ? `${r.scaling.min_flavor} ×${r.scaling.min_instances}`
        : `${r.scaling.min_flavor}..${r.scaling.max_flavor} ×${r.scaling.min_instances}..${r.scaling.max_instances}`;
      nodes.push({ id: r.id, kind: r.kind === 'task' ? 'task' : 'app', label: r.service, sub: `${image} · ${scale}${r.run === 'local' ? ' · locale' : ''}`, status: 'planned', data: r });
      for (const d of r.depends_on as string[]) edges.push({ from: d, to: r.id, kind: d.startsWith('addon/') ? 'link' : 'depends' });
    } else if (r.resource === 'addon') {
      const engine = r.options?.documentdb ? ' + DocumentDB' : '';
      nodes.push({ id: r.id, kind: 'addon', label: r.service, sub: `${providerLabel(r.provider)}${engine} ${r.version || ''} · ${r.plan}${r.private ? '' : ' · hôte public'}`, status: 'planned', data: r });
    } else if (r.resource === 'network_group') {
      groups.push({ id: r.id, label: r.name, members: [] });
    }
  }
  for (const r of plan.resources) {
    if (r.resource === 'membership') {
      const g = groups.find((g) => g.id === r.group);
      if (g) g.members.push(r.member);
    }
  }
  for (const r of plan.resources) {
    if (r.resource === 'domain') {
      const n = nodes.find((n) => n.id === r.app);
      if (n) n.sub += ` · ${r.fqdn}`;
    }
    if (r.resource === 'redirection') {
      const n = nodes.find((n) => n.id === r.app);
      if (n) n.sub += ` · ${r.protocol} ${r.target_port}`;
    }
  }
  return { nodes, groups, edges };
}

export function graphFromInventory(inv: Inventory): Graph {
  const nodes: GraphNode[] = [];
  const groups: GraphGroup[] = [];
  const edges: GraphEdge[] = [];
  const byPlatformId = new Map<string, string>();
  for (const a of inv.apps) {
    const up = a.instances.filter((i) => i.state === 'UP').length;
    const deploying = a.instances.some((i) => i.state !== 'UP');
    let status: NodeStatus = 'stopped';
    if (a.instances.length && !deploying) status = 'up';
    else if (deploying) status = 'deploying';
    else if (a.last_deployment?.state === 'FAIL' || a.last_deployment?.state === 'CANCELLED') status = 'failed';
    if (a.task) status = a.last_deployment?.state === 'OK' ? 'done' : status;
    const id = `app/${a.service}`;
    const scale = `${a.scaling.min_flavor}${a.scaling.min_flavor !== a.scaling.max_flavor ? '..' + a.scaling.max_flavor : ''} ×${a.scaling.min_instances}${a.scaling.min_instances !== a.scaling.max_instances ? '..' + a.scaling.max_instances : ''}`;
    nodes.push({ id, kind: a.task ? 'task' : 'app', label: a.service, sub: `${a.instances.length ? up + ' instance(s)' : (a.last_deployment ? 'dernier déploiement ' + a.last_deployment.state.toLowerCase() : 'jamais déployée')} · ${scale}${a.tagged ? '' : ' · sans tag'}`, status, data: a });
    byPlatformId.set(a.id, id);
  }
  for (const a of inv.addons) {
    const id = `addon/${a.service}`;
    nodes.push({ id, kind: 'addon', label: a.service, sub: `${providerLabel(a.provider)} · ${a.plan}`, status: 'up', data: a });
    byPlatformId.set(a.real_id, id);
    byPlatformId.set(a.id, id);
  }
  for (const g of inv.network_groups) {
    groups.push({ id: g.id, label: g.label, members: g.members.map((m) => byPlatformId.get(m.id)).filter((x): x is string => !!x) });
  }
  return { nodes, groups, edges };
}

export interface Position {
  x: number;
  y: number;
}

export interface Box {
  id: string;
  label: string;
  members: string[];
  kind: 'group' | 'loose';
  x: number;
  y: number;
  width: number;
  height: number;
}

export interface Layout {
  W: number;
  H: number;
  boxes: Box[];
  positions: Map<string, Position>;
  width: number;
  height: number;
}

/** Disposition : chaque network group est une boîte, les nœuds y sont
 * rangés par profondeur de dépendance (colonnes) ; les nœuds hors groupe
 * vont dans une boîte « hors network group ». */
export function layout(graph: Graph): Layout {
  const W = 200;
  const H = 66;
  const GX = 40;
  const GY = 22;
  const PAD = 18;
  const TITLE = 28;
  const depth = new Map<string, number>();
  const deps = (id: string) => graph.edges.filter((e) => e.to === id).map((e) => e.from);
  const depthOf = (id: string, seen = new Set<string>()): number => {
    if (depth.has(id)) return depth.get(id)!;
    if (seen.has(id)) return 0;
    seen.add(id);
    const d = deps(id).reduce((m, f) => Math.max(m, depthOf(f, seen) + 1), 0);
    depth.set(id, d);
    return d;
  };
  for (const n of graph.nodes) depthOf(n.id);

  const placed = new Map<string, Position>();
  const boxes: Box[] = [];
  let y = 0;
  const inGroup = new Set(graph.groups.flatMap((g) => g.members));
  const containers: Omit<Box, 'x' | 'y' | 'width' | 'height'>[] = graph.groups.map((g) => ({ id: g.id, label: g.label, members: g.members, kind: 'group' as const }));
  const loose = graph.nodes.filter((n) => !inGroup.has(n.id)).map((n) => n.id);
  if (loose.length) containers.push({ id: 'loose', label: 'hors network group', members: loose, kind: 'loose' });
  let maxWidth = 0;
  for (const c of containers) {
    const members = c.members.map((id) => graph.nodes.find((n) => n.id === id)).filter((n): n is GraphNode => !!n);
    const cols = new Map<number, GraphNode[]>();
    for (const n of members) {
      const d = depth.get(n.id) || 0;
      if (!cols.has(d)) cols.set(d, []);
      cols.get(d)!.push(n);
    }
    const colKeys = [...cols.keys()].sort((a, b) => a - b);
    let rows = 0;
    colKeys.forEach((d, ci) => {
      cols.get(d)!.forEach((n, ri) => {
        placed.set(n.id, { x: PAD + ci * (W + GX), y: y + TITLE + PAD + ri * (H + GY) });
        rows = Math.max(rows, ri + 1);
      });
    });
    const title = (c.kind === 'group' ? 'network group · ' : '') + c.label;
    const width = Math.max(PAD * 2 + Math.max(1, colKeys.length) * W + Math.max(0, colKeys.length - 1) * GX, title.length * 7.5 + 40);
    const height = TITLE + PAD * 2 + Math.max(1, rows) * H + Math.max(0, rows - 1) * GY;
    boxes.push({ ...c, x: 0, y, width, height });
    maxWidth = Math.max(maxWidth, width);
    y += height + 30;
  }
  for (const b of boxes) b.width = maxWidth;
  return { W, H, boxes, positions: placed, width: maxWidth + 2, height: Math.max(y - 30, 80) };
}
