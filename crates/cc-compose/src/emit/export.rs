//! `clever-compose export` : écrit les deux émissions et les dépôts
//! synthétiques dans un répertoire.

use std::path::Path as FsPath;

use cc_compose_model::plan::{Plan, Resource};

use super::{script, terraform};
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::plan::sources;

pub struct What {
    pub terraform: bool,
    pub script: bool,
    pub repos: bool,
    /// Écrire les branches `clever-compose/<service>` dans le dépôt git du
    /// projet (pour `deployment.repository`).
    pub branches: bool,
}

pub fn write(plan: &Plan, project_dir: &FsPath, out_dir: &FsPath, what: &What, diags: &mut Diagnostics) -> Result<(), Diagnostic> {
    std::fs::create_dir_all(out_dir).map_err(|e| Diagnostic::error("export-io", format!("{} : {e}", out_dir.display())))?;
    let io = |p: &FsPath, e: std::io::Error| Diagnostic::error("export-io", format!("{} : {e}", p.display()));
    let mut origin: Option<String> = None;
    if what.branches {
        for r in &plan.resources {
            let Resource::App(app) = r else { continue };
            let files = repo_files(app, project_dir).map_err(|e| Diagnostic::error("export-sources", format!("{} : {e}", app.service)))?;
            match crate::exec::git::write_branch(project_dir, &app.service, &files) {
                Ok((built, o)) => {
                    let oid = built.oid();
                    // La plateforme clone en HTTPS : `git@host:x/y.git` → `https://host/x/y.git`.
                    origin = origin.or(o.map(|u| match u.strip_prefix("git@").and_then(|r| r.split_once(':')) {
                        Some((host, path)) => format!("https://{host}/{path}"),
                        None => u,
                    }));
                    // La branche est prolongée, pas réécrite : son
                    // historique est celui des déploiements.
                    let state = match built {
                        crate::exec::git::Built::Unchanged(_) => "inchangée",
                        crate::exec::git::Built::Commit(_) => "prolongée",
                    };
                    diags.push(Diagnostic::warning("export-branch", format!("branche clever-compose/{} {state} ({}) ; à pousser : git push origin 'refs/heads/clever-compose/*'", app.service, &oid.to_string()[..12])));
                }
                Err(e) => diags.push(Diagnostic::warning("export-branch", e)),
            }
        }
    }
    if what.terraform {
        // HCL lisible pour l'utilisateur ; la structure JSON reste celle qui
        // est validée en test.
        let tf = terraform::emit(plan, &terraform::Options { out_dir, project_dir, repository: origin.clone() }, diags);
        let p = out_dir.join("main.tf");
        std::fs::write(&p, super::hcl::render(&tf, &terraform::schema())).map_err(|e| io(&p, e))?;
    }
    if what.script {
        let p = out_dir.join("bootstrap.sh");
        std::fs::write(&p, script::emit(plan, &script::Options { out_dir, project_dir }, diags)).map_err(|e| io(&p, e))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755));
        }
    }
    if what.repos {
        for r in &plan.resources {
            let Resource::App(app) = r else { continue };
            let dir = out_dir.join("repos").join(&app.service);
            std::fs::create_dir_all(&dir).map_err(|e| io(&dir, e))?;
            // Même contenu que ce que l'exécuteur pousse (contexte de build
            // filtré par .dockerignore compris).
            let files = repo_files(app, project_dir).map_err(|e| Diagnostic::error("export-sources", format!("{} : {e}", app.service)))?;
            for f in files {
                let p = dir.join(&f.path);
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
                }
                std::fs::write(&p, &f.content).map_err(|e| io(&p, e))?;
                #[cfg(unix)]
                if f.executable {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755));
                }
            }
        }
    }
    Ok(())
}


/// Fichiers du dépôt synthétique, contexte de build compris.
fn repo_files(app: &cc_compose_model::plan::App, project_dir: &FsPath) -> Result<Vec<sources::File>, String> {
    crate::exec::reconcile::repo_files(app, project_dir)
}
