//! Exécution distante : la CLI parle à un serveur `clever-compose api`
//! (`--api-url`, `CC_API_URL`) au lieu d'appeler la plateforme elle-même.
//! Ce sont les identifiants du serveur qui sont utilisés ; la CLI envoie
//! son organisation dans `X-CC-Org` et, si le serveur l'exige, la clé
//! `CC_API_KEY` en `Authorization: Bearer`. Un projet local est envoyé en
//! archive `.tar.gz` (sans `.git`, `node_modules`, `target`), extrait dans
//! un répertoire temporaire du serveur et retiré après l'opération. Les
//! lignes du journal SSE sont rendues comme en local.

use std::path::Path;

use reqwest;
use serde_json::{json, Value as J};

use crate::exec::reconcile::Log;

pub struct Remote {
    base: String,
    key: Option<String>,
    org: Option<String>,
    /// Les jetons du poste, envoyés à chaque requête : le serveur agit en
    /// notre nom, jamais avec les siens.
    tokens: Option<clevercloud_sdk::Credentials>,
    http: reqwest::Client,
}

impl Remote {
    pub fn new(url: &str, key: Option<String>, org: Option<String>, tokens: Option<clevercloud_sdk::Credentials>) -> Result<Self, String> {
        let base = url.trim_end_matches('/').to_string();
        if !(base.starts_with("http://") || base.starts_with("https://")) {
            return Err(format!("--api-url `{url}` : URL http(s) attendue"));
        }
        if base.starts_with("http://") && !(base.starts_with("http://127.0.0.1") || base.starts_with("http://localhost") || base.starts_with("http://[::1]")) {
            eprintln!("avertissement : {base} sans TLS, vos jetons partent en clair");
        }
        Ok(Remote { base, key, org: org.filter(|o| !o.is_empty()), tokens, http: reqwest::Client::new() })
    }

    fn req(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut r = self.http.request(method, format!("{}{path}", self.base));
        if let Some(k) = &self.key {
            r = r.bearer_auth(k);
        }
        if let Some(o) = &self.org {
            r = r.header("X-CC-Org", o);
        }
        match &self.tokens {
            Some(clevercloud_sdk::Credentials::OAuth1 { token, secret, consumer_key, consumer_secret }) => r = r.header("X-CC-Token", token).header("X-CC-Secret", secret).header("X-CC-Consumer-Key", consumer_key).header("X-CC-Consumer-Secret", consumer_secret),
            Some(clevercloud_sdk::Credentials::Bearer { token }) => r = r.header("X-CC-Api-Token", token),
            _ => {}
        }
        r
    }

    async fn check(resp: reqwest::Response) -> Result<reqwest::Response, String> {
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let text = resp.text().await.unwrap_or_default();
        let msg = serde_json::from_str::<J>(&text).ok().and_then(|v| v["error"].as_str().map(str::to_string)).unwrap_or(text);
        Err(format!("serveur : {status} {msg}"))
    }

    pub async fn json(&self, method: reqwest::Method, path: &str, body: Option<J>) -> Result<J, String> {
        let mut r = self.req(method, path);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let resp = Self::check(r.send().await.map_err(|e| format!("{} : {e}", self.base))?).await?;
        resp.json().await.map_err(|e| e.to_string())
    }

    /// Flux SSE `log`/`done` d'une opération longue, rendu ligne à ligne.
    pub async fn stream(&self, method: reqwest::Method, path: &str, body: Option<J>, log: &mut Log) -> Result<(), String> {
        let mut r = self.req(method, path);
        if let Some(b) = body {
            r = r.json(&b);
        }
        let mut resp = Self::check(r.send().await.map_err(|e| format!("{} : {e}", self.base))?).await?;
        let mut buf = String::new();
        let mut event = String::new();
        let mut data: Option<String> = None;
        let mut outcome: Option<Result<(), String>> = None;
        while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
            buf.push_str(&String::from_utf8_lossy(&chunk));
            while let Some(nl) = buf.find('\n') {
                let line = buf[..nl].trim_end_matches('\r').to_string();
                buf.drain(..=nl);
                if let Some(v) = line.strip_prefix("event:") {
                    event = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("data:") {
                    let v = v.strip_prefix(' ').unwrap_or(v);
                    data = Some(match data.take() {
                        Some(mut d) => {
                            d.push('\n');
                            d.push_str(v);
                            d
                        }
                        None => v.to_string(),
                    });
                } else if line.is_empty() {
                    if let Some(d) = data.take() {
                        match event.as_str() {
                            "done" => outcome = Some(if d == "ok" { Ok(()) } else { Err(d.strip_prefix("error: ").unwrap_or(&d).to_string()) }),
                            _ => log(&d),
                        }
                    }
                    event.clear();
                }
            }
            if outcome.is_some() {
                break;
            }
        }
        outcome.unwrap_or_else(|| Err("flux interrompu avant la fin de l'opération".into()))
    }

    /// Envoie le répertoire du projet et rend l'identifiant du projet
    /// temporaire côté serveur.
    pub async fn upload_project(&self, dir: &Path, name: &str) -> Result<String, String> {
        let bytes = archive(dir)?;
        let part = reqwest::multipart::Part::bytes(bytes).file_name(format!("{name}.tar.gz")).mime_str("application/gzip").map_err(|e| e.to_string())?;
        let form = reqwest::multipart::Form::new().part("file", part);
        let resp = Self::check(self.req(reqwest::Method::POST, "/api/local/upload").multipart(form).send().await.map_err(|e| e.to_string())?).await?;
        let v: J = resp.json().await.map_err(|e| e.to_string())?;
        v["id"].as_str().map(str::to_string).ok_or_else(|| "réponse inattendue à l'envoi du projet".into())
    }

    pub async fn upload_file(&self, id: &str, path: &Path) -> Result<String, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{} : {e}", path.display()))?;
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("dump").to_string();
        let part = reqwest::multipart::Part::bytes(bytes).file_name(name);
        let form = reqwest::multipart::Form::new().part("file", part);
        let resp = Self::check(self.req(reqwest::Method::POST, &format!("/api/local/{}/seed/upload", enc(id))).multipart(form).send().await.map_err(|e| e.to_string())?).await?;
        let v: J = resp.json().await.map_err(|e| e.to_string())?;
        v["path"].as_str().map(str::to_string).ok_or_else(|| "réponse inattendue à l'envoi du dump".into())
    }

    pub async fn forget_project(&self, id: &str) {
        let _ = self.req(reqwest::Method::DELETE, &format!("/api/local/{}", enc(id))).send().await;
    }

    pub fn project_path(name: &str, op: &str) -> String {
        format!("/api/platform/projects/{}/{op}", enc(name))
    }

    pub fn local_path(id: &str, op: &str) -> String {
        format!("/api/local/{}/{op}", enc(id))
    }

    pub async fn projects(&self) -> Result<Vec<String>, String> {
        let v = self.json(reqwest::Method::GET, "/api/platform/projects", None).await?;
        Ok(v["projects"].as_array().into_iter().flatten().filter_map(|p| p.as_str().map(str::to_string)).collect())
    }
}

pub fn enc(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.~".contains(c) { c.to_string() } else { c.to_string().bytes().map(|b| format!("%{b:02X}")).collect() }).collect()
}

/// Le projet en `.tar.gz`, hors `.git`, `node_modules`, `target`,
/// `.terraform` et fichiers de plus de 64 Mio.
fn archive(dir: &Path) -> Result<Vec<u8>, String> {
    let enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut tar = tar::Builder::new(enc);
    tar.follow_symlinks(false);
    fn walk(tar: &mut tar::Builder<flate2::write::GzEncoder<Vec<u8>>>, root: &Path, dir: &Path) -> Result<(), String> {
        let mut entries: Vec<_> = std::fs::read_dir(dir).map_err(|e| format!("{} : {e}", dir.display()))?.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            let rel = p.strip_prefix(root).unwrap_or(&p).to_path_buf();
            if p.is_dir() {
                if matches!(name.as_str(), ".git" | "node_modules" | "target" | ".terraform") {
                    continue;
                }
                tar.append_dir(&rel, &p).map_err(|e| e.to_string())?;
                walk(tar, root, &p)?;
            } else if p.is_file() {
                if e.metadata().map(|m| m.len() > 64 * 1024 * 1024).unwrap_or(true) {
                    continue;
                }
                tar.append_path_with_name(&p, &rel).map_err(|e| format!("{} : {e}", p.display()))?;
            }
        }
        Ok(())
    }
    walk(&mut tar, dir, dir)?;
    let enc = tar.into_inner().map_err(|e| e.to_string())?;
    enc.finish().map_err(|e| e.to_string())
}

/// Ce que `main` demande au serveur.
pub enum Op {
    Up { yes: bool, dry_run: bool, no_wait: bool, allow_ephemeral: bool, assets: bool },
    Seed { service: String, dump: std::path::PathBuf, local: bool, yes: bool, no_wait: bool },
    Down { all: bool, dry_run: bool, yes: bool },
    Stop,
    Start,
    Ps,
    Logs { services: Vec<String>, since: Option<String>, limit: Option<usize>, follow: bool },
    Export { terraform: bool, script: bool, allow_ephemeral: bool, output: std::path::PathBuf },
    Orgs,
}

/// Exécute une opération contre le serveur. `project_dir` et `project`
/// (nom) viennent des options du compose comme en local.
pub async fn run(remote: &Remote, op: Op, project_dir: &Path, project: &str, log: &mut Log) -> Result<(), String> {
    const GET: reqwest::Method = reqwest::Method::GET;
    const POST: reqwest::Method = reqwest::Method::POST;
    match op {
        Op::Up { yes, dry_run, no_wait, allow_ephemeral, assets } => {
            log(&format!("envoi du projet à {}", remote.base));
            let id = remote.upload_project(project_dir, project).await?;
            let r = remote.stream(POST, &Remote::local_path(&id, "up"), Some(json!({ "yes": yes, "dry_run": dry_run, "no_wait": no_wait, "allow_ephemeral": allow_ephemeral, "assets": assets })), log).await;
            remote.forget_project(&id).await;
            r
        }
        Op::Seed { service, dump, local, yes, no_wait } => {
            if local {
                return Err("seed --local s'exécute sur le poste : incompatible avec --api-url".into());
            }
            log(&format!("envoi du projet et du dump à {}", remote.base));
            let id = remote.upload_project(project_dir, project).await?;
            let r = async {
                let path = remote.upload_file(&id, &dump).await?;
                remote.stream(POST, &Remote::local_path(&id, "seed"), Some(json!({ "service": service, "dump": path, "local": false, "yes": yes, "no_wait": no_wait })), log).await
            }
            .await;
            remote.forget_project(&id).await;
            r
        }
        Op::Down { all, dry_run, yes } => {
            let names = if all { remote.projects().await? } else { vec![project.to_string()] };
            if names.is_empty() {
                log("aucun projet clever-compose sur la plateforme");
            }
            for name in names {
                remote.stream(POST, &Remote::project_path(&name, "down"), Some(json!({ "dry_run": dry_run, "yes": yes })), log).await?;
            }
            Ok(())
        }
        Op::Stop => remote.stream(POST, &Remote::project_path(project, "stop"), None, log).await,
        Op::Start => remote.stream(POST, &Remote::project_path(project, "start"), None, log).await,
        Op::Ps => remote.stream(POST, &Remote::project_path(project, "ps"), None, log).await,
        Op::Logs { services, since, limit, follow } => {
            let mut q = vec![format!("follow={follow}")];
            if !services.is_empty() {
                q.push(format!("services={}", enc(&services.join(","))));
            }
            if let Some(s) = since {
                q.push(format!("since={}", enc(&s)));
            }
            if let Some(n) = limit {
                q.push(format!("limit={n}"));
            }
            remote.stream(GET, &format!("{}?{}", Remote::project_path(project, "logs"), q.join("&")), None, log).await
        }
        Op::Export { terraform, script, allow_ephemeral, output } => {
            log(&format!("envoi du projet à {}", remote.base));
            let id = remote.upload_project(project_dir, project).await?;
            let r = remote.json(POST, &Remote::local_path(&id, "export"), Some(json!({ "branches": false, "allow_ephemeral": allow_ephemeral }))).await;
            remote.forget_project(&id).await;
            let v = r?;
            for d in v["diagnostics"].as_array().into_iter().flatten() {
                log(&format!("avertissement : {}", d.as_str().unwrap_or("")));
            }
            let mut n = 0;
            for f in v["files"].as_array().into_iter().flatten() {
                let (Some(path), Some(content)) = (f["path"].as_str(), f["content"].as_str()) else { continue };
                let is_script = path == "bootstrap.sh";
                let keep = (terraform && !is_script) || (script && is_script);
                if !keep {
                    continue;
                }
                let target = output.join(path);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                std::fs::write(&target, content).map_err(|e| format!("{} : {e}", target.display()))?;
                n += 1;
            }
            log(&format!("{n} fichiers écrits dans {} (les dépôts synthétiques et --branches ne sont pas exportés à distance)", output.display()));
            Ok(())
        }
        Op::Orgs => {
            let v = remote.json(GET, "/api/orgs", None).await?;
            let orgs: Vec<(String, String)> = v["orgs"].as_array().into_iter().flatten().map(|o| (o["id"].as_str().unwrap_or("").to_string(), o["name"].as_str().unwrap_or("").to_string())).collect();
            let w = orgs.iter().map(|(id, _)| id.len()).max().unwrap_or(0);
            for (id, name) in orgs {
                println!("{id:<w$}  {name}");
            }
            Ok(())
        }
    }
}
