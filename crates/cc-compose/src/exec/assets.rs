//! Assets d'un projet dans un bucket Cellar (S3) : les fichiers du projet
//! (compose, fichiers montés, scripts), le plan et un manifeste, poussés
//! par `up` quand le projet le demande (`x-clever.assets: true` ou
//! `--assets`). Un projet déployé se relit alors depuis la plateforme sans
//! son dossier : l'interface montre le compose et le plan, et peut le
//! rapatrier en projet local.
//!
//! Un addon Cellar par organisation (`clever-compose-assets`, plan `S`,
//! facturé à l'usage), un bucket par projet. Requêtes S3 signées par
//! `rusty-s3` (URL présignées), envoyées avec `reqwest`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use cc_compose_model::plan::*;
use clevercloud_sdk::oauth10a::reqwest;
use rusty_s3::actions::S3Action;
use rusty_s3::{Bucket, Credentials, UrlStyle};
use serde::{Deserialize, Serialize};

use super::platform::{NewAddon, Platform};
use super::reconcile::Log;

pub const ADDON_NAME: &str = "clever-compose-assets";
pub const PROVIDER: &str = "cellar-addon";

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

/// Nom du bucket d'un projet : le projet et un fragment de l'organisation
/// (les noms de buckets sont partagés par tous les clients du cluster).
pub fn bucket_name(project: &str, owner: &str) -> String {
    let frag: String = owner.split_once('_').map(|(_, id)| id).unwrap_or(owner).chars().filter(|c| c.is_ascii_hexdigit()).take(8).collect::<String>().to_lowercase();
    let proj: String = project.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    format!("cc-{proj}-{frag}").chars().take(63).collect()
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

    pub async fn ensure_bucket(&self) -> Result<bool, String> {
        if self.bucket_exists().await? {
            return Ok(false);
        }
        let url = self.bucket.create_bucket(&self.creds).sign(EXPIRES);
        let resp = self.http.put(url).send().await.map_err(|e| format!("Cellar : {e}"))?;
        self.check("création du bucket", resp).await?;
        Ok(true)
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

    pub async fn list(&self) -> Result<Vec<(String, u64)>, String> {
        let mut out = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut action = self.bucket.list_objects_v2(Some(&self.creds));
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

    /// Vide puis supprime le bucket.
    pub async fn destroy(&self) -> Result<(), String> {
        for (key, _) in self.list().await? {
            self.delete(&key).await?;
        }
        let url = self.bucket.delete_bucket(&self.creds).sign(EXPIRES);
        let resp = self.http.delete(url).send().await.map_err(|e| format!("Cellar : {e}"))?;
        self.check("suppression du bucket", resp).await?;
        Ok(())
    }
}

/// Les fichiers du projet à conserver : fichiers texte (compose, env,
/// scripts, configs) jusqu'à 512 Kio, et tout fichier référencé par le plan
/// (bind mounts, scripts d'init) jusqu'à 32 Mio ; jamais `.git`,
/// `node_modules`, `target`, `.terraform`, ni `.clever-compose/`.
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

/// L'addon Cellar de l'organisation, créé si absent (avec `confirm`), et
/// ses variables.
pub async fn cellar_for<P: Platform>(platform: &P, region: &str, confirm: bool, dry_run: bool, log: &mut Log) -> Result<Option<Vec<super::platform::EnvVar>>, String> {
    let existing = platform.list_addons().await?;
    let addon = match existing.iter().find(|a| a.name == ADDON_NAME && a.provider == PROVIDER) {
        Some(a) => a.id.clone(),
        None if dry_run => {
            log(&format!("addon {ADDON_NAME} (Cellar) : à créer pour les assets"));
            return Ok(None);
        }
        None if !confirm => return Err(format!("l'addon Cellar `{ADDON_NAME}` (assets des projets, facturé à l'usage) serait créé : relancez avec `--yes`")),
        None => {
            let created = platform.create_addon(&NewAddon { name: ADDON_NAME.into(), provider: PROVIDER.into(), plan: "S".into(), region: region.to_string(), version: None, options: Default::default() }).await?;
            log(&format!("addon {ADDON_NAME} (Cellar) : créé ({}) pour les assets des projets", created.id));
            created.id
        }
    };
    Ok(Some(platform.addon_env(&addon).await?))
}

/// Pousse les assets du projet dans son bucket ; supprime ce qui n'y est plus.
pub async fn sync<P: Platform>(plan: &Plan, project_dir: &Path, platform: &P, region: &str, opts: &super::reconcile::Options<'_>, log: &mut Log) -> Result<(), String> {
    let owner = platform.owner_id();
    let files = project_files(plan, project_dir);
    if opts.dry_run {
        log(&format!("assets : {} fichier(s) + plan.json à pousser dans le bucket {}", files.len(), bucket_name(&plan.project, &owner)));
        return Ok(());
    }
    let Some(env) = cellar_for(platform, region, opts.confirm_addons, opts.dry_run, log).await? else { return Ok(()) };
    let cellar = Cellar::from_env(&env, &bucket_name(&plan.project, &owner))?;
    if cellar.ensure_bucket().await? {
        log(&format!("assets : bucket {} créé", cellar.bucket_name()));
    }
    let existing: std::collections::BTreeMap<String, u64> = cellar.list().await?.into_iter().collect();
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
        let key = format!("files/{rel}");
        wanted.insert(key.clone());
        // Même taille et manifeste précédent identique : on saute.
        let unchanged = existing.get(&key).is_some_and(|s| *s == bytes.len() as u64) && previous_sha(&cellar, &key).await.as_deref() == Some(sha.as_str());
        if !unchanged {
            cellar.put(&key, bytes.clone()).await?;
            pushed += 1;
        }
        manifest.files.push(ManifestFile { path: rel.clone(), size: bytes.len() as u64, sha256: sha });
    }
    cellar.put("plan.json", serde_json::to_vec_pretty(plan).unwrap()).await?;
    cellar.put("manifest.json", serde_json::to_vec_pretty(&manifest).unwrap()).await?;
    wanted.insert("plan.json".into());
    wanted.insert("manifest.json".into());
    let mut removed = 0;
    for key in existing.keys() {
        if !wanted.contains(key) {
            cellar.delete(key).await?;
            removed += 1;
        }
    }
    log(&format!("assets : {} fichier(s) dans le bucket {} ({pushed} poussé(s), {removed} retiré(s))", files.len(), cellar.bucket_name()));
    Ok(())
}

/// Le sha256 d'un fichier d'après le manifeste déjà en place.
async fn previous_sha(cellar: &Cellar, key: &str) -> Option<String> {
    let bytes = cellar.get("manifest.json").await.ok()??;
    let m: Manifest = serde_json::from_slice(&bytes).ok()?;
    let rel = key.strip_prefix("files/")?;
    m.files.iter().find(|f| f.path == rel).map(|f| f.sha256.clone())
}

/// Le bucket d'un projet, s'il existe (addon Cellar de l'organisation).
pub async fn open<P: Platform>(platform: &P, owner: &str, project: &str) -> Result<Option<Cellar>, String> {
    let existing = platform.list_addons().await?;
    let Some(addon) = existing.iter().find(|a| a.name == ADDON_NAME && a.provider == PROVIDER) else { return Ok(None) };
    let env = platform.addon_env(&addon.id).await?;
    let cellar = Cellar::from_env(&env, &bucket_name(project, owner))?;
    if !cellar.bucket_exists().await? {
        return Ok(None);
    }
    Ok(Some(cellar))
}

/// Manifeste, plan et fichiers d'un projet depuis son bucket.
pub async fn fetch(cellar: &Cellar) -> Result<(Manifest, Option<Plan>, Vec<(String, Vec<u8>)>), String> {
    let manifest: Manifest = match cellar.get("manifest.json").await? {
        Some(b) => serde_json::from_slice(&b).map_err(|e| format!("manifest.json : {e}"))?,
        None => return Err("bucket sans manifest.json".into()),
    };
    let plan = cellar.get("plan.json").await?.and_then(|b| serde_json::from_slice(&b).ok());
    let mut files = Vec::new();
    for f in &manifest.files {
        if let Some(b) = cellar.get(&format!("files/{}", f.path)).await? {
            files.push((f.path.clone(), b));
        }
    }
    Ok((manifest, plan, files))
}

/// Supprime le bucket d'un projet, s'il existe.
pub async fn remove<P: Platform>(platform: &P, owner: &str, project: &str, dry_run: bool, log: &mut Log) -> Result<(), String> {
    let Some(cellar) = open(platform, owner, project).await? else { return Ok(()) };
    if dry_run {
        log(&format!("assets : bucket {} à supprimer", cellar.bucket_name()));
        return Ok(());
    }
    cellar.destroy().await?;
    log(&format!("assets : bucket {} supprimé", cellar.bucket_name()));
    Ok(())
}
