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
    let ctx = Context { images, project_dir: &project_dir, spec_json: &spec, allow_ephemeral: opts.allow_ephemeral };
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
