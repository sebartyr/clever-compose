//! Lecture de la configuration d'une image sur son registre, sans démon
//! Docker (`oci-client`) : manifeste, puis blob de configuration, dont on
//! garde entrypoint, cmd, user, workdir et ports exposés. Un index
//! multi-plateformes est résolu en `linux/amd64` (CLAUDE.md §2 : amd64
//! seul pour l'instant).
//!
//! Identifiants : `~/.docker/config.json` (`auths`, sans credential
//! helpers) ; sinon anonyme. Cache disque par référence sous
//! `$XDG_CACHE_HOME/clever-compose/images/`, invalidé pour un tag mutable
//! (`latest` ou sans tag) après `CLEVER_COMPOSE_IMAGE_CACHE_TTL` secondes (défaut
//! 86400), jamais pour un digest.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use cc_compose_model::plan::ImageIntrinsics;
use oci_client::client::{linux_amd64_resolver, ClientConfig};
use oci_client::secrets::RegistryAuth;
use oci_client::{Client, Reference};
use serde::{Deserialize, Serialize};

use super::images::ImageResolver;

pub struct Registry {
    runtime: tokio::runtime::Runtime,
    client: Client,
    auths: HashMap<String, (String, String)>,
    cache_dir: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
struct Cached {
    fetched_at: u64,
    digest: String,
    intrinsics: ImageIntrinsics,
}

impl Registry {
    /// Avec les identifiants Docker du poste (`~/.docker/config.json`).
    pub fn new() -> Result<Self, String> {
        Self::build(docker_auths())
    }

    /// Sans aucun identifiant : pour un projet déposé sur un serveur, dont
    /// les images ne doivent pas se lire avec les identifiants du serveur.
    pub fn anonymous() -> Result<Self, String> {
        Self::build(HashMap::new())
    }

    fn build(auths: HashMap<String, (String, String)>) -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|e| format!("runtime : {e}"))?;
        let config = ClientConfig { platform_resolver: Some(Box::new(linux_amd64_resolver)), ..ClientConfig::default() };
        let client = Client::new(config);
        let cache_dir = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .map(|c| c.join("clever-compose").join("images"));
        Ok(Registry { runtime, client, auths, cache_dir })
    }

    fn auth_for(&self, registry: &str) -> RegistryAuth {
        match credentials_for(&self.auths, registry) {
            Some((u, p)) => RegistryAuth::Basic(u, p),
            None => RegistryAuth::Anonymous,
        }
    }

    fn cache_path(&self, image: &str) -> Option<PathBuf> {
        let dir = self.cache_dir.as_ref()?;
        let safe: String = image.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' }).collect();
        Some(dir.join(format!("{safe}.json")))
    }

    fn fetch(&self, image: &str) -> Result<(String, ImageIntrinsics), String> {
        let reference: Reference = image.parse().map_err(|e| format!("référence d'image invalide `{image}` : {e}"))?;
        let auth = self.auth_for(reference.resolve_registry());
        let (_, digest, config) = self
            .runtime
            .block_on(self.client.pull_manifest_and_config(&reference, &auth))
            .map_err(|e| format!("registre `{}` : {e}", reference.resolve_registry()))?;
        let file: oci_client::config::ConfigFile = serde_json::from_str(&config).map_err(|e| format!("configuration d'image invalide : {e}"))?;
        let c = file.config.unwrap_or_default();
        let mut exposed: Vec<u16> = c.exposed_ports.unwrap_or_default().iter().filter_map(|p| p.split('/').next()?.parse().ok()).collect();
        exposed.sort_unstable();
        exposed.dedup();
        let intr = ImageIntrinsics {
            entrypoint: c.entrypoint.unwrap_or_default(),
            cmd: c.cmd.unwrap_or_default(),
            user: c.user.filter(|u| !u.is_empty()),
            workdir: c.working_dir.filter(|w| !w.is_empty()),
            exposed_ports: exposed,
        };
        Ok((digest, intr))
    }
}

impl ImageResolver for Registry {
    fn resolve(&self, image: &str) -> Result<ImageIntrinsics, String> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let ttl: u64 = std::env::var("CLEVER_COMPOSE_IMAGE_CACHE_TTL").ok().and_then(|v| v.parse().ok()).unwrap_or(86_400);
        let pinned = image.contains("@sha256:");
        let cache = self.cache_path(image);
        if let Some(path) = &cache {
            if let Ok(text) = std::fs::read_to_string(path) {
                if let Ok(c) = serde_json::from_str::<Cached>(&text) {
                    if pinned || now.saturating_sub(c.fetched_at) < ttl {
                        return Ok(c.intrinsics);
                    }
                }
            }
        }
        let (digest, intrinsics) = self.fetch(image)?;
        if let Some(path) = &cache {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(path, serde_json::to_string(&Cached { fetched_at: now, digest, intrinsics: intrinsics.clone() }).unwrap_or_default());
        }
        Ok(intrinsics)
    }
}

/// Identifiants d'un registre dans les `auths` de Docker (Docker Hub sous
/// ses différents noms).
pub fn credentials_for(auths: &HashMap<String, (String, String)>, registry: &str) -> Option<(String, String)> {
    let keys: Vec<&str> = match registry {
        "docker.io" | "index.docker.io" | "registry-1.docker.io" => vec!["https://index.docker.io/v1/", "index.docker.io", "docker.io", "registry-1.docker.io"],
        r => vec![r],
    };
    keys.iter().find_map(|k| auths.get(*k).cloned())
}

/// Hôte du registre d'une référence d'image (`ghcr.io/o/i:t` → `ghcr.io`).
pub fn registry_host(image: &str) -> String {
    let first = image.split('/').next().unwrap_or("");
    let has_host = image.contains('/') && (first.contains('.') || first.contains(':') || first == "localhost");
    if has_host { first.to_string() } else { "docker.io".into() }
}

/// `auths` de `~/.docker/config.json` (ou `$DOCKER_CONFIG/config.json`) :
/// `auth` en base64 `user:password`, ou `username`/`password` en clair.
pub fn docker_auths() -> HashMap<String, (String, String)> {
    use base64::Engine;
    let mut out = HashMap::new();
    let path = std::env::var_os("DOCKER_CONFIG")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".docker")))
        .map(|d| d.join("config.json"));
    let Some(path) = path else { return out };
    let Ok(text) = std::fs::read_to_string(path) else { return out };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else { return out };
    let Some(auths) = json.get("auths").and_then(|a| a.as_object()) else { return out };
    for (registry, v) in auths {
        let creds = if let Some(auth) = v.get("auth").and_then(|a| a.as_str()) {
            base64::engine::general_purpose::STANDARD
                .decode(auth)
                .ok()
                .and_then(|b| String::from_utf8(b).ok())
                .and_then(|s| s.split_once(':').map(|(u, p)| (u.to_string(), p.to_string())))
        } else {
            match (v.get("username").and_then(|u| u.as_str()), v.get("password").and_then(|p| p.as_str())) {
                (Some(u), Some(p)) => Some((u.to_string(), p.to_string())),
                _ => None,
            }
        };
        if let Some(c) = creds {
            let key = registry.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/').to_string();
            out.insert(registry.clone(), c.clone());
            out.insert(key, c);
        }
    }
    out
}
