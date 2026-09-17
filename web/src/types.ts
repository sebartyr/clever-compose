// Formes des réponses de l'API du serveur, telles que `web/api.rs` les
// sérialise. Le plan et l'inventaire sont du JSON côté Rust
// (`cc-compose-model`), gardés souples ici : seuls les champs que
// l'interface lit sont nommés.

export type Json = Record<string, any>;

export interface Session {
  authenticated: boolean;
  email?: string | null;
  org: string;
  own_tokens?: boolean;
  reason?: string;
}

export interface Info {
  version?: string;
  commit?: string;
  built?: string;
  assets_bucket?: string | null;
  api_key?: boolean;
  server_credentials?: boolean;
  local_exec?: boolean;
}

export interface Org {
  id: string;
  name: string;
  personal?: boolean;
}

export interface LocalProject {
  id: string;
  dir: string;
  temporary: boolean;
  name: string;
}

/** Projet ouvert : local (déposé ou du poste) ou sur la plateforme. */
export interface Current {
  type: 'local' | 'platform';
  id: string;
  name: string;
  dir?: string;
}

export interface Diagnostic {
  severity: 'error' | 'warning';
  code: string;
  message: string;
  path?: string | null;
  file?: string | null;
  line?: number | null;
  col?: number | null;
  rendered: string;
}

export interface Scaling {
  min_flavor: string;
  max_flavor: string;
  min_instances: number;
  max_instances: number;
}

export interface Flavor {
  name: string;
  cpus: number;
  memory_bytes: number;
}

export interface Flavors {
  default: string;
  max_instances: number;
  flavors: Flavor[];
}

/** Une ressource du plan (`Resource` côté Rust, étiquetée `resource`). */
export type PlanResource = Json & { resource: string; id: string };

export interface Plan {
  project: string;
  spec_hash: string;
  resources: PlanResource[];
}

export interface InventoryApp extends Json {
  service: string;
  id: string;
  name: string;
  task: boolean;
  tagged: boolean;
  state: string;
  instances: { id: string; state: string; flavor: string | null }[];
  scaling: Scaling;
  last_deployment: { state: string; id: string; date: number } | null;
  domains: string[];
  tags: string[];
}

export interface InventoryAddon extends Json {
  service: string;
  id: string;
  real_id: string;
  name: string;
  provider: string;
  plan: string;
}

export interface Inventory {
  project: string;
  apps: InventoryApp[];
  addons: InventoryAddon[];
  network_groups: { id: string; label: string; members: { id: string; domain_name: string }[] }[];
}

export interface AssetFile {
  path: string;
  size: number;
  content: string | null;
}

export interface Assets {
  present: boolean;
  configured: boolean;
  bucket?: string;
  manifest?: { project: string; spec_hash: string; uploaded_at: string; files: { path: string; size: number; sha256: string }[] };
  plan?: Plan | null;
  files?: AssetFile[];
}

export interface ExportResult {
  files: { path: string; content: string }[];
  diagnostics: string[];
}

export interface ScalingBody extends Scaling {
  service: string;
}

// ------------------------------------------------------------ diagramme

export type NodeKind = 'app' | 'task' | 'addon';
export type NodeStatus = 'planned' | 'up' | 'deploying' | 'failed' | 'stopped' | 'done';

export interface GraphNode {
  id: string;
  kind: NodeKind;
  label: string;
  sub: string;
  status: NodeStatus;
  data: Json;
}

export interface GraphGroup {
  id: string;
  label: string;
  members: string[];
}

export interface GraphEdge {
  from: string;
  to: string;
  kind: 'depends' | 'link';
}

export interface Graph {
  nodes: GraphNode[];
  groups: GraphGroup[];
  edges: GraphEdge[];
}
