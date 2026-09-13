//! Modèle typé du projet Compose canonique, limité à ce que le
//! planificateur consomme. Toute clé non modélisée arrive dans `extra` et
//! reçoit un diagnostic : rien n'est approximé en silence (CLAUDE.md §13).

use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::Value as J;

#[derive(Debug, Clone, Deserialize)]
pub struct Project {
    pub name: String,
    #[serde(default)]
    pub services: IndexMap<String, Service>,
    #[serde(default)]
    pub networks: IndexMap<String, Network>,
    #[serde(default)]
    pub volumes: IndexMap<String, Volume>,
    #[serde(default)]
    pub secrets: IndexMap<String, Secret>,
    #[serde(default)]
    pub configs: IndexMap<String, J>,
    #[serde(flatten)]
    pub extra: IndexMap<String, J>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Service {
    pub image: Option<String>,
    pub build: Option<Build>,
    #[serde(default)]
    pub command: Option<Vec<String>>,
    #[serde(default)]
    pub entrypoint: Option<Vec<String>>,
    #[serde(default)]
    pub environment: IndexMap<String, Option<String>>,
    #[serde(default)]
    pub ports: Vec<Port>,
    #[serde(default)]
    pub expose: Vec<String>,
    #[serde(default)]
    pub depends_on: IndexMap<String, Dependency>,
    pub healthcheck: Option<HealthCheck>,
    pub deploy: Option<Deploy>,
    #[serde(default)]
    pub volumes: Vec<VolumeMount>,
    #[serde(default)]
    pub secrets: Vec<SecretMount>,
    #[serde(default)]
    pub networks: IndexMap<String, Option<ServiceNetwork>>,
    pub restart: Option<String>,
    pub user: Option<String>,
    pub working_dir: Option<String>,
    #[serde(default)]
    pub labels: IndexMap<String, String>,
    #[serde(default)]
    pub profiles: Vec<String>,
    pub container_name: Option<String>,
    pub hostname: Option<String>,
    pub scale: Option<i64>,
    pub mem_limit: Option<String>,
    pub cpus: Option<f64>,
    pub stop_grace_period: Option<String>,
    pub pull_policy: Option<String>,
    pub platform: Option<String>,
    pub init: Option<bool>,
    pub tty: Option<bool>,
    pub stdin_open: Option<bool>,
    pub read_only: Option<bool>,
    pub logging: Option<J>,
    #[serde(default)]
    pub links: Vec<String>,
    #[serde(default)]
    pub extra_hosts: Vec<String>,
    #[serde(default)]
    pub dns: Vec<String>,
    #[serde(default)]
    pub dns_search: Vec<String>,
    #[serde(default)]
    pub tmpfs: Vec<String>,
    #[serde(default)]
    pub cap_add: Vec<String>,
    #[serde(default)]
    pub cap_drop: Vec<String>,
    pub privileged: Option<bool>,
    #[serde(default)]
    pub devices: Vec<J>,
    pub network_mode: Option<String>,
    #[serde(default)]
    pub sysctls: IndexMap<String, String>,
    #[serde(default)]
    pub security_opt: Vec<String>,
    pub pid: Option<String>,
    pub ipc: Option<String>,
    pub shm_size: Option<String>,
    pub ulimits: Option<J>,
    #[serde(default)]
    pub configs: Vec<J>,
    #[serde(default)]
    pub annotations: IndexMap<String, String>,
    pub stop_signal: Option<String>,
    pub oom_kill_disable: Option<bool>,
    pub oom_score_adj: Option<i64>,
    pub cpu_shares: Option<i64>,
    pub cpu_count: Option<i64>,
    pub cpu_percent: Option<f64>,
    pub cpuset: Option<String>,
    pub mem_reservation: Option<String>,
    pub memswap_limit: Option<String>,
    pub mem_swappiness: Option<i64>,
    pub pids_limit: Option<i64>,
    pub runtime: Option<String>,
    pub isolation: Option<String>,
    pub cgroup: Option<String>,
    pub cgroup_parent: Option<String>,
    pub userns_mode: Option<String>,
    pub uts: Option<String>,
    pub domainname: Option<String>,
    pub mac_address: Option<String>,
    #[serde(default)]
    pub group_add: Vec<String>,
    #[serde(default)]
    pub volumes_from: Vec<String>,
    #[serde(default)]
    pub external_links: Vec<String>,
    pub develop: Option<J>,
    pub attach: Option<bool>,
    pub provider: Option<J>,
    pub models: Option<J>,
    pub gpus: Option<J>,
    pub post_start: Option<J>,
    pub pre_stop: Option<J>,
    pub pre_start: Option<J>,
    pub label_file: Option<J>,
    pub credential_spec: Option<J>,
    pub blkio_config: Option<J>,
    pub storage_opt: Option<J>,
    pub device_cgroup_rules: Option<J>,
    pub cpu_rt_runtime: Option<J>,
    pub cpu_rt_period: Option<J>,
    pub cpu_period: Option<J>,
    pub cpu_quota: Option<J>,
    pub use_api_socket: Option<bool>,
    #[serde(flatten)]
    pub extra: IndexMap<String, J>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Build {
    pub context: String,
    pub dockerfile: Option<String>,
    pub dockerfile_inline: Option<String>,
    #[serde(default)]
    pub args: IndexMap<String, Option<String>>,
    pub target: Option<String>,
    #[serde(flatten)]
    pub extra: IndexMap<String, J>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Port {
    pub target: u16,
    pub published: Option<String>,
    pub host_ip: Option<String>,
    #[serde(default = "default_tcp")]
    pub protocol: String,
    #[serde(default)]
    pub mode: Option<String>,
    pub name: Option<String>,
    pub app_protocol: Option<String>,
}

fn default_tcp() -> String {
    "tcp".into()
}

#[derive(Debug, Clone, Deserialize)]
pub struct Dependency {
    #[serde(default = "default_condition")]
    pub condition: String,
    #[serde(default = "default_true")]
    pub required: bool,
    #[serde(default)]
    pub restart: bool,
}

fn default_condition() -> String {
    "service_started".into()
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct HealthCheck {
    #[serde(default)]
    pub test: Vec<String>,
    pub interval: Option<String>,
    pub timeout: Option<String>,
    pub retries: Option<i64>,
    pub start_period: Option<String>,
    pub start_interval: Option<String>,
    pub disable: Option<bool>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Deploy {
    pub replicas: Option<i64>,
    pub resources: Option<Resources>,
    pub mode: Option<String>,
    #[serde(default)]
    pub labels: IndexMap<String, String>,
    pub restart_policy: Option<J>,
    pub update_config: Option<J>,
    pub rollback_config: Option<J>,
    pub placement: Option<J>,
    pub endpoint_mode: Option<String>,
    #[serde(flatten)]
    pub extra: IndexMap<String, J>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Resources {
    pub limits: Option<ResourceSpec>,
    pub reservations: Option<ResourceSpec>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ResourceSpec {
    pub cpus: Option<J>,
    pub memory: Option<String>,
    pub pids: Option<i64>,
    pub devices: Option<J>,
    pub generic_resources: Option<J>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VolumeMount {
    #[serde(rename = "type")]
    pub kind: String,
    pub source: Option<String>,
    pub target: String,
    #[serde(default)]
    pub read_only: bool,
    pub bind: Option<J>,
    pub volume: Option<J>,
    pub tmpfs: Option<J>,
    pub consistency: Option<String>,
    pub image: Option<J>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SecretMount {
    pub source: String,
    pub target: Option<String>,
    pub uid: Option<String>,
    pub gid: Option<String>,
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ServiceNetwork {
    #[serde(default)]
    pub aliases: Vec<String>,
    pub ipv4_address: Option<String>,
    pub ipv6_address: Option<String>,
    pub priority: Option<i64>,
    #[serde(default)]
    pub link_local_ips: Vec<String>,
    pub mac_address: Option<String>,
    pub gw_priority: Option<i64>,
    pub interface_name: Option<String>,
    pub driver_opts: Option<J>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Network {
    pub name: Option<String>,
    pub driver: Option<String>,
    pub external: Option<bool>,
    pub internal: Option<bool>,
    pub attachable: Option<bool>,
    pub ipam: Option<J>,
    #[serde(default)]
    pub labels: IndexMap<String, String>,
    pub driver_opts: Option<J>,
    pub enable_ipv4: Option<bool>,
    pub enable_ipv6: Option<bool>,
    #[serde(flatten)]
    pub extra: IndexMap<String, J>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Volume {
    pub name: Option<String>,
    pub driver: Option<String>,
    pub external: Option<bool>,
    pub driver_opts: Option<J>,
    #[serde(default)]
    pub labels: IndexMap<String, String>,
    #[serde(flatten)]
    pub extra: IndexMap<String, J>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Secret {
    pub name: Option<String>,
    pub file: Option<String>,
    pub environment: Option<String>,
    pub external: Option<bool>,
    #[serde(default)]
    pub labels: IndexMap<String, String>,
    #[serde(flatten)]
    pub extra: IndexMap<String, J>,
}

impl Project {
    pub fn from_tree(tree: &super::node::Node) -> Result<Project, serde_json::Error> {
        serde_json::from_value(tree.to_json())
    }
}
