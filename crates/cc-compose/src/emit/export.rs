//! `clever-compose export` : écrit les deux émissions et les dépôts
//! synthétiques dans un répertoire.

use std::path::Path as FsPath;

use cc_compose_model::plan::{Plan, Resource, Source};

use super::{script, terraform};
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::plan::sources;

pub struct What {
    pub terraform: bool,
    pub script: bool,
    pub repos: bool,
}

pub fn write(plan: &Plan, project_dir: &FsPath, out_dir: &FsPath, what: &What, diags: &mut Diagnostics) -> Result<(), Diagnostic> {
    std::fs::create_dir_all(out_dir).map_err(|e| Diagnostic::error("export-io", format!("{} : {e}", out_dir.display())))?;
    let io = |p: &FsPath, e: std::io::Error| Diagnostic::error("export-io", format!("{} : {e}", p.display()));
    if what.terraform {
        let tf = terraform::emit(plan, &terraform::Options { out_dir, project_dir }, diags);
        let p = out_dir.join("main.tf.json");
        std::fs::write(&p, serde_json::to_string_pretty(&tf).unwrap() + "\n").map_err(|e| io(&p, e))?;
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
            // Contexte de build de l'utilisateur, copié tel quel (sans .git).
            if let Source::Context { context, .. } = &app.source {
                let src = project_dir.join(context.trim_start_matches("./"));
                copy_tree(&src, &dir).map_err(|e| Diagnostic::error("export-io", format!("{} : {e}", src.display())))?;
            }
            let files = sources::files(app, project_dir).map_err(|e| Diagnostic::error("export-sources", format!("{} : {e}", app.service)))?;
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

fn copy_tree(src: &FsPath, dst: &FsPath) -> std::io::Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let target = dst.join(&name);
        if entry.file_type()?.is_dir() {
            std::fs::create_dir_all(&target)?;
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}
