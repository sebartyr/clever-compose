//! Assets d'un projet dans un bucket Cellar (S3) : les fichiers du projet
//! (compose, fichiers montés, scripts), le plan et un manifeste, poussés
//! par `up` quand le projet le demande (`x-clever.assets: true` ou
//! `--assets`). Un projet déployé se relit alors depuis la plateforme sans
//! son dossier : l'interface montre le compose et le plan, et peut le
//! rapatrier en projet local.
//!
//! L'addon Cellar et le bucket sont créés par l'utilisateur, jamais par
//! l'outil, et configurés par variables d'environnement :
//! `CELLAR_ADDON_HOST`, `CELLAR_ADDON_KEY_ID`, `CELLAR_ADDON_KEY_SECRET`
//! (les noms que l'addon injecte) et `CLEVER_COMPOSE_ASSETS_BUCKET` (un seul bucket,
//! un préfixe `<propriétaire>/<projet>/` par projet : le bucket peut servir
//! plusieurs organisations, un client ne lit que sous la sienne). `down`
//! retire les objets du projet, jamais le bucket. Requêtes S3 signées par
//! `rusty-s3` (URL présignées), envoyées avec `reqwest`.
//!
//! Les fichiers déclarés comme secrets (`secrets.*.file`) ne sont jamais
//! poussés : ils vivent sur le poste, le shim les matérialise depuis
//! l'environnement de l'application.

use std::path::{Path, PathBuf};
use std::time::Duration;

use cc_compose_model::plan::*;
use clevercloud_sdk::oauth10a::reqwest;
use rusty_s3::actions::S3Action;
use rusty_s3::{Bucket, Credentials, UrlStyle};
use serde::{Deserialize, Serialize};

use super::reconcile::Log;

/// Variables de configuration des assets.
pub const ENV_BUCKET: &str = "CLEVER_COMPOSE_ASSETS_BUCKET";
pub const ENV_HOST: &str = "CELLAR_ADDON_HOST";
pub const ENV_KEY: &str = "CELLAR_ADDON_KEY_ID";
pub const ENV_SECRET: &str = "CELLAR_ADDON_KEY_SECRET";

/// Ce que le bucket décrit de lui-même (`manifest.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub project: String,
    pub spec_hash: String,
    /// RFC 3339.
    pub uploaded_at: String,
    pub files: Vec<ManifestFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

pub struct Cellar {
    bucket: Bucket,
    creds: Credentials,
    http: reqwest::Client,
}

const EXPIRES: Duration = Duration::from_secs(600);

/// Configuration lue dans l'environnement ; `None` si les assets ne sont
/// pas configurés, erreur si la configuration est incomplète.
pub fn from_env() -> Result<Option<Cellar>, String> {
    let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    let Some(bucket) = get(ENV_BUCKET) else { return Ok(None) };
    match (get(ENV_HOST), get(ENV_KEY), get(ENV_SECRET)) {
        (Some(h), Some(k), Some(s)) => Cellar::new(&h, &k, &s, &bucket).map(Some),
        _ => Err(format!("{ENV_BUCKET} est posée mais {ENV_HOST}, {ENV_KEY} ou {ENV_SECRET} manque (variables de l'addon Cellar)")),
    }
}

/// Préfixe des objets d'un projet dans le bucket : sous son propriétaire
/// (`orga_…` ou `user_…`), pour qu'un bucket partagé entre organisations
/// ne mélange pas leurs projets.
fn prefix(owner: &str, project: &str) -> String {
    format!("{owner}/{project}/")
}

impl Cellar {
    pub fn new(host: &str, key: &str, secret: &str, bucket: &str) -> Result<Cellar, String> {
        let endpoint = format!("https://{host}").parse().map_err(|e| format!("Cellar : {e}"))?;
        let bucket = Bucket::new(endpoint, UrlStyle::Path, bucket.to_string(), "us-east-1").map_err(|e| format!("Cellar : {e}"))?;
        Ok(Cellar { bucket, creds: Credentials::new(key, secret), http: reqwest::Client::new() })
    }

    /// Depuis les variables de l'addon (`CELLAR_ADDON_*`).
    pub fn from_env(env: &[super::platform::EnvVar], bucket: &str) -> Result<Cellar, String> {
        let get = |k: &str| env.iter().find(|e| e.name == k).map(|e| e.value.clone()).ok_or_else(|| format!("addon Cellar : variable {k} absente"));
        Cellar::new(&get("CELLAR_ADDON_HOST")?, &get("CELLAR_ADDON_KEY_ID")?, &get("CELLAR_ADDON_KEY_SECRET")?, bucket)
    }

    pub fn bucket_name(&self) -> &str {
        self.bucket.name()
    }

    pub fn host(&self) -> String {
        self.bucket.base_url().host_str().unwrap_or_default().to_string()
    }

    async fn check(&self, what: &str, resp: reqwest::Response) -> Result<reqwest::Response, String> {
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let text = resp.text().await.unwrap_or_default();
        Err(format!("Cellar {what} : {status} {}", text.chars().take(300).collect::<String>()))
    }

    pub async fn bucket_exists(&self) -> Result<bool, String> {
        let url = self.bucket.head_bucket(Some(&self.creds)).sign(EXPIRES);
        let resp = self.http.head(url).send().await.map_err(|e| format!("Cellar : {e}"))?;
        Ok(resp.status().is_success())
    }

    pub async fn put(&self, key: &str, bytes: Vec<u8>) -> Result<(), String> {
        let url = self.bucket.put_object(Some(&self.creds), key).sign(EXPIRES);
        let resp = self.http.put(url).body(bytes).send().await.map_err(|e| format!("Cellar : {e}"))?;
        self.check(&format!("écriture de {key}"), resp).await?;
        Ok(())
    }

    pub async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        let url = self.bucket.get_object(Some(&self.creds), key).sign(EXPIRES);
        let resp = self.http.get(url).send().await.map_err(|e| format!("Cellar : {e}"))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = self.check(&format!("lecture de {key}"), resp).await?;
        Ok(Some(resp.bytes().await.map_err(|e| e.to_string())?.to_vec()))
    }

    pub async fn list(&self, prefix: &str) -> Result<Vec<(String, u64)>, String> {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut action = self.bucket.list_objects_v2(Some(&self.creds));
            action.with_prefix(prefix.to_string());
            if let Some(t) = &token {
                action.with_continuation_token(t.clone());
            }
            let url = action.sign(EXPIRES);
            let resp = self.http.get(url).send().await.map_err(|e| format!("Cellar : {e}"))?;
            let resp = self.check("liste des objets", resp).await?;
            let text = resp.text().await.map_err(|e| e.to_string())?;
            let parsed = rusty_s3::actions::ListObjectsV2::parse_response(&text).map_err(|e| format!("Cellar liste : {e}"))?;
            out.extend(parsed.contents.into_iter().map(|c| (c.key, c.size)));
            match parsed.next_continuation_token {
                Some(t) => token = Some(t),
                None => break,
            }
        }
        Ok(out)
    }

    pub async fn delete(&self, key: &str) -> Result<(), String> {
        let url = self.bucket.delete_object(Some(&self.creds), key).sign(EXPIRES);
        let resp = self.http.delete(url).send().await.map_err(|e| format!("Cellar : {e}"))?;
        self.check(&format!("suppression de {key}"), resp).await?;
        Ok(())
    }

    /// Retire les objets d'un projet ; le bucket reste.
    pub async fn clear(&self, owner: &str, project: &str) -> Result<usize, String> {
        let keys = self.list(&prefix(owner, project)).await?;
        for (key, _) in &keys {
            self.delete(key).await?;
        }
        Ok(keys.len())
    }
}

/// Les fichiers du projet à conserver : fichiers texte (compose, env,
/// scripts, configs) jusqu'à 512 Kio, et tout fichier référencé par le plan
/// (bind mounts, scripts d'init) jusqu'à 32 Mio ; jamais `.git`,
/// `node_modules`, `target`, `.terraform`, `.clever-compose/`, ni un
/// fichier déclaré comme secret (`secrets.*.file`).
pub fn project_files(plan: &Plan, project_dir: &Path) -> Vec<(String, PathBuf)> {
    fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<(String, PathBuf)>) {
        if depth > 6 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                if matches!(name.as_str(), ".git" | "node_modules" | "target" | ".terraform" | ".clever-compose") {
                    continue;
                }
                walk(root, &p, depth + 1, out);
            } else if p.is_file() && e.metadata().map(|m| m.len() <= 512 * 1024).unwrap_or(false) {
                if let Ok(bytes) = std::fs::read(&p) {
                    if std::str::from_utf8(&bytes).is_ok() && !bytes.contains(&0) {
                        out.push((p.strip_prefix(root).unwrap_or(&p).to_string_lossy().to_string(), p.clone()));
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(project_dir, project_dir, 0, &mut out);
    let secret_files: Vec<PathBuf> = plan
        .resources
        .iter()
        .filter_map(|r| if let Resource::App(a) = r { Some(a) } else { None })
        .flat_map(|a| a.secrets.iter())
        .filter_map(|s| match &s.source {
            SecretSource::File { path } => Some(project_dir.join(path.trim_start_matches("./"))),
            SecretSource::Environment { .. } => None,
        })
        .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
        .collect();
    out.retain(|(_, abs)| !secret_files.contains(&std::fs::canonicalize(abs).unwrap_or_else(|_| abs.clone())));
    for r in &plan.resources {
        let Resource::App(a) = r else { continue };
        let files = match &a.source {
            Source::Synthetic { files, .. } | Source::Context { files, .. } => files,
        };
        for f in files {
            let rel = f.source.trim_start_matches("./").to_string();
            let abs = project_dir.join(&rel);
            if abs.is_file() && !out.iter().any(|(r, _)| *r == rel) && abs.metadata().map(|m| m.len() <= 32 * 1024 * 1024).unwrap_or(false) {
                out.push((rel, abs));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Pousse les assets du projet sous son préfixe ; retire ce qui n'y est plus.
pub async fn sync(owner: &str, plan: &Plan, project_dir: &Path, opts: &super::reconcile::Options<'_>, log: &mut Log) -> Result<(), String> {
    let Some(cellar) = from_env()? else {
        return Err(format!("assets demandés mais non configurés : posez {ENV_BUCKET} et les variables de l'addon Cellar ({ENV_HOST}, {ENV_KEY}, {ENV_SECRET})"));
    };
    let files = project_files(plan, project_dir);
    let pre = prefix(owner, &plan.project);
    if opts.dry_run {
        log(&format!("assets : {} fichier(s) + plan.json à pousser dans {}/{pre}", files.len(), cellar.bucket_name()));
        return Ok(());
    }
    if !cellar.bucket_exists().await? {
        return Err(format!("assets : le bucket `{}` n'existe pas sur {} (créez-le dans l'addon Cellar)", cellar.bucket_name(), cellar.host()));
    }
    let existing: std::collections::BTreeMap<String, u64> = cellar.list(&pre).await?.into_iter().collect();
    let previous: Option<Manifest> = cellar.get(&format!("{pre}manifest.json")).await?.and_then(|b| serde_json::from_slice(&b).ok());
    let mut manifest = Manifest { project: plan.project.clone(), spec_hash: plan.spec_hash.clone(), uploaded_at: super::inspect::rfc3339(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()), files: Vec::new() };
    let mut wanted: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut pushed = 0;
    for (rel, abs) in &files {
        let bytes = std::fs::read(abs).map_err(|e| format!("{} : {e}", abs.display()))?;
        let sha = {
            use sha2::Digest;
            let mut h = sha2::Sha256::new();
            h.update(&bytes);
            h.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>()
        };
        let key = format!("{pre}files/{rel}");
        wanted.insert(key.clone());
        // Même taille et même empreinte que dans le manifeste en place : sauté.
        let unchanged = existing.get(&key).is_some_and(|s| *s == bytes.len() as u64) && previous.as_ref().and_then(|m| m.files.iter().find(|f| f.path == *rel)).is_some_and(|f| f.sha256 == sha);
        if !unchanged {
            cellar.put(&key, bytes.clone()).await?;
            pushed += 1;
        }
        manifest.files.push(ManifestFile { path: rel.clone(), size: bytes.len() as u64, sha256: sha });
    }
    cellar.put(&format!("{pre}plan.json"), serde_json::to_vec_pretty(plan).unwrap()).await?;
    cellar.put(&format!("{pre}manifest.json"), serde_json::to_vec_pretty(&manifest).unwrap()).await?;
    wanted.insert(format!("{pre}plan.json"));
    wanted.insert(format!("{pre}manifest.json"));
    let mut removed = 0;
    for key in existing.keys() {
        if !wanted.contains(key) {
            cellar.delete(key).await?;
            removed += 1;
        }
    }
    log(&format!("assets : {} fichier(s) dans {}/{pre} ({pushed} poussé(s), {removed} retiré(s))", files.len(), cellar.bucket_name()));
    Ok(())
}

/// Les assets d'un projet, si les assets sont configurés et que le projet
/// en a (un manifeste sous son préfixe).
pub async fn open(owner: &str, project: &str) -> Result<Option<(Cellar, Manifest)>, String> {
    let Some(cellar) = from_env()? else { return Ok(None) };
    let Some(bytes) = cellar.get(&format!("{}manifest.json", prefix(owner, project))).await? else { return Ok(None) };
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|e| format!("manifest.json : {e}"))?;
    Ok(Some((cellar, manifest)))
}

/// Plan et fichiers d'un projet depuis le bucket.
pub async fn fetch(cellar: &Cellar, owner: &str, manifest: &Manifest) -> Result<(Option<Plan>, Vec<(String, Vec<u8>)>), String> {
    let pre = prefix(owner, &manifest.project);
    let plan = cellar.get(&format!("{pre}plan.json")).await?.and_then(|b| serde_json::from_slice(&b).ok());
    let mut files = Vec::new();
    for f in &manifest.files {
        if let Some(b) = cellar.get(&format!("{pre}files/{}", f.path)).await? {
            files.push((f.path.clone(), b));
        }
    }
    Ok((plan, files))
}

/// Retire les objets d'un projet, si les assets sont configurés.
pub async fn remove(owner: &str, project: &str, dry_run: bool, log: &mut Log) -> Result<(), String> {
    let Some(cellar) = from_env()? else { return Ok(()) };
    let pre = prefix(owner, project);
    let keys = cellar.list(&pre).await?;
    if keys.is_empty() {
        return Ok(());
    }
    if dry_run {
        log(&format!("assets : {} objet(s) à retirer de {}/{pre}", keys.len(), cellar.bucket_name()));
        return Ok(());
    }
    let n = cellar.clear(owner, project).await?;
    log(&format!("assets : {n} objet(s) retiré(s) de {}/{pre}", cellar.bucket_name()));
    Ok(())
}
