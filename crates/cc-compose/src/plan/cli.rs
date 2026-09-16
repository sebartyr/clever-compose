//! Chargement d'un projet puis planification, partagé par la CLI et les
//! tests golden.

use std::path::Path as FsPath;

use cc_compose_model::plan::Plan;

use super::images::{Fixture, ImageResolver};
use super::{plan, Context};
use crate::compose::model::Project;
use crate::compose::project::{self, Options};
use crate::diagnostic::{Diagnostic, Diagnostics};

pub struct PlanOptions {
    pub project: Options,
    pub allow_ephemeral: bool,
    /// Hôtes de registre avec identifiants sur le poste ; `None` = lus dans
    /// la configuration Docker (`private_registries_from_docker`).
    pub private_registries: Option<Vec<String>>,
}

/// Les hôtes des `auths` de la configuration Docker du poste, normalisés
/// (Docker Hub → `docker.io`).
pub fn private_registries_from_docker() -> Vec<String> {
    let auths = super::registry::docker_auths();
    let mut out: Vec<String> = auths
        .keys()
        .map(|k| {
            let k = k.trim_start_matches("https://").trim_start_matches("http://").trim_end_matches('/');
            match k {
                "index.docker.io/v1" | "index.docker.io" | "registry-1.docker.io" => "docker.io".to_string(),
                other => other.to_string(),
            }
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

pub fn run(opts: &PlanOptions, images: &dyn ImageResolver, diags: &mut Diagnostics) -> Option<Plan> {
    let loaded = project::load(&opts.project, diags)?;
    if diags.has_errors() {
        return None;
    }
    let spec = loaded.tree.to_json();
    let typed = match Project::from_tree(&loaded.tree) {
        Ok(t) => t,
        Err(e) => {
            diags.push(Diagnostic::error("model", format!("modèle Compose inattendu : {e}")));
            return None;
        }
    };
    let project_dir = std::fs::canonicalize(&opts.project.project_dir).unwrap_or_else(|_| opts.project.project_dir.clone());
    let private_registries = opts.private_registries.clone().unwrap_or_else(private_registries_from_docker);
    let ctx = Context { images, project_dir: &project_dir, spec_json: &spec, allow_ephemeral: opts.allow_ephemeral, private_registries: &private_registries };
    let before = diags.items.len();
    let result = plan(&typed, &ctx, diags);
    // Les diagnostics du plan reçoivent la position du nœud désigné par leur
    // chemin (l'arbre canonique garde les positions d'origine), et sont
    // triés par chemin pour une sortie stable.
    let mut tail = diags.items.split_off(before);
    for d in tail.iter_mut() {
        if d.span.is_none() {
            if let Some(p) = &d.path {
                d.span = crate::compose::schema::find_span(&loaded.tree, p.segments());
            }
        }
    }
    tail.sort_by_key(|d| (d.path.as_ref().map(|p| p.to_string()).unwrap_or_default(), d.code));
    diags.items.extend(tail);
    result
}

pub fn fixture_from_file(path: &FsPath) -> Result<Fixture, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("{} : {e}", path.display()))?;
    serde_json::from_str(&content).map_err(|e| format!("{} : {e}", path.display()))
}

/// Fixture d'images d'un projet : chaque `image:` résolue par `resolver`.
/// Les services `build:` n'en font pas partie (leur Dockerfile est lu).
pub fn images_fixture(opts: &Options, resolver: &dyn ImageResolver, diags: &mut Diagnostics) -> Option<Fixture> {
    let loaded = project::load(opts, diags)?;
    let typed = Project::from_tree(&loaded.tree).ok()?;
    let mut out = Fixture::default();
    for s in typed.services.values() {
        let Some(image) = &s.image else { continue };
        if s.build.is_some() || out.0.contains_key(image) {
            continue;
        }
        match resolver.resolve(image) {
            Ok(i) => {
                out.0.insert(image.clone(), i);
            }
            Err(e) => diags.push(Diagnostic::error("image-unresolved", format!("{image} : {e}"))),
        }
    }
    Some(out)
}
