//! Configuration des images : ce que l'image déclare (entrypoint, cmd,
//! user, workdir, ports exposés), lu sur le registre au plan (CLAUDE.md §8)
//! ou fourni par une fixture pour les tests hors ligne.

use std::path::Path as FsPath;

use cc_compose_model::plan::ImageIntrinsics;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

pub trait ImageResolver {
    fn resolve(&self, image: &str) -> Result<ImageIntrinsics, String>;
}

/// Fixture : table image → configuration, chargée d'un JSON.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Fixture(pub IndexMap<String, ImageIntrinsics>);

impl ImageResolver for Fixture {
    fn resolve(&self, image: &str) -> Result<ImageIntrinsics, String> {
        self.0.get(image).cloned().ok_or_else(|| format!("image `{image}` absente de la fixture"))
    }
}

/// Un registre public : Docker Hub sans identifiants, ghcr.io, quay.io...
/// Tout hôte inconnu est supposé privé (identifiants via `CC_DOCKER_LOGIN_*`).
pub fn is_public_registry(image: &str) -> bool {
    let Some((first, _)) = image.split_once('/') else {
        // `image:tag` sans chemin : Docker Hub.
        return true;
    };
    let has_host = first.contains('.') || first.contains(':') || first == "localhost";
    if !has_host {
        return true;
    }
    matches!(first, "docker.io" | "registry-1.docker.io" | "ghcr.io" | "quay.io" | "gcr.io" | "registry.gitlab.com" | "public.ecr.aws" | "mcr.microsoft.com" | "lscr.io")
}

/// Configuration résultant d'un Dockerfile : les directives de la dernière
/// étape, héritées de son image de base.
pub fn from_dockerfile(context: &FsPath, dockerfile: &str, resolver: &dyn ImageResolver) -> Result<ImageIntrinsics, String> {
    let path = context.join(dockerfile);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{} : {e}", path.display()))?;
    let mut stages: Vec<(String, ImageIntrinsics, Option<String>)> = Vec::new(); // (from, config, alias)
    let mut current: Option<(String, ImageIntrinsics, Option<String>)> = None;
    let mut args: IndexMap<String, String> = IndexMap::new();
    for raw in logical_lines(&text) {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (instr, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        match instr.to_ascii_uppercase().as_str() {
            "ARG" => {
                if let Some((k, v)) = rest.split_once('=') {
                    args.entry(k.trim().to_string()).or_insert(v.trim().trim_matches('"').to_string());
                }
            }
            "FROM" => {
                if let Some(c) = current.take() {
                    stages.push(c);
                }
                let mut parts = rest.split_whitespace().filter(|w| !w.starts_with("--"));
                let mut from = parts.next().unwrap_or("").to_string();
                for (k, v) in &args {
                    from = from.replace(&format!("${{{k}}}"), v).replace(&format!("${k}"), v);
                }
                let alias = parts.skip_while(|w| !w.eq_ignore_ascii_case("AS")).nth(1).map(str::to_string);
                let base = if let Some(st) = stages.iter().find(|(_, _, a)| a.as_deref() == Some(from.as_str())) {
                    st.1.clone()
                } else if from == "scratch" {
                    ImageIntrinsics { entrypoint: Vec::new(), cmd: Vec::new(), user: None, workdir: None, exposed_ports: Vec::new() }
                } else {
                    resolver.resolve(&from)?
                };
                // Une nouvelle étape hérite de l'image de base, sauf le CMD si
                // l'ENTRYPOINT est redéfini plus loin.
                current = Some((from, base, alias));
            }
            "ENTRYPOINT" => {
                if let Some((_, c, _)) = current.as_mut() {
                    c.entrypoint = exec_form(rest);
                    c.cmd = Vec::new();
                }
            }
            "CMD" => {
                if let Some((_, c, _)) = current.as_mut() {
                    c.cmd = exec_form(rest);
                }
            }
            "USER" => {
                if let Some((_, c, _)) = current.as_mut() {
                    c.user = Some(rest.to_string());
                }
            }
            "WORKDIR" => {
                if let Some((_, c, _)) = current.as_mut() {
                    c.workdir = Some(rest.to_string());
                }
            }
            "EXPOSE" => {
                if let Some((_, c, _)) = current.as_mut() {
                    for w in rest.split_whitespace() {
                        if let Ok(p) = w.split('/').next().unwrap_or("").parse::<u16>() {
                            if !c.exposed_ports.contains(&p) {
                                c.exposed_ports.push(p);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(c) = current.take() {
        stages.push(c);
    }
    stages.pop().map(|(_, c, _)| c).ok_or_else(|| format!("{} : aucune instruction FROM", path.display()))
}

/// Lignes logiques : continuation par `\` en fin de ligne.
fn logical_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut acc = String::new();
    for line in text.lines() {
        if let Some(stripped) = line.trim_end().strip_suffix('\\') {
            acc.push_str(stripped);
            acc.push(' ');
            continue;
        }
        acc.push_str(line);
        out.push(std::mem::take(&mut acc));
    }
    if !acc.is_empty() {
        out.push(acc);
    }
    out
}

/// `ENTRYPOINT ["a", "b"]` (forme exec) ou `ENTRYPOINT a b` (forme shell,
/// équivalente à `["/bin/sh", "-c", "a b"]`).
fn exec_form(rest: &str) -> Vec<String> {
    if rest.starts_with('[') {
        if let Ok(v) = serde_json::from_str::<Vec<String>>(rest) {
            return v;
        }
    }
    if rest.is_empty() {
        return Vec::new();
    }
    vec!["/bin/sh".into(), "-c".into(), rest.to_string()]
}
