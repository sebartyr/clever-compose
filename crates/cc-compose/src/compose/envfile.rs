//! Fusion des `env_file` dans `environment`
//! (`Project.WithServicesEnvironmentResolved` de compose-go, avec
//! `discardEnvFiles`) : les variables du fichier sont surchargées par celles
//! du service ; la substitution dans le fichier voit d'abord l'environnement
//! du projet, puis l'environnement du service, puis le fichier lui-même.

use indexmap::IndexMap;

use super::dotenv::{self, Vars};
use super::node::{Node, Path, Value};
use crate::diagnostic::Diagnostic;

pub fn resolve(tree: &mut Node, project_env: &Vars, diags: &mut Vec<Diagnostic>) {
    let span = tree.span;
    let Some(services) = tree.as_map_mut().and_then(|m| m.get_mut("services")).and_then(Node::as_map_mut) else { return };
    for (name, service) in services.iter_mut() {
        let Some(s) = service.as_map_mut() else { continue };
        let Some(files) = s.shift_remove("env_file") else { continue };
        let path = Path::root().child("services").child(name).child("env_file");
        let service_env: IndexMap<String, Node> = s.get("environment").and_then(Node::as_map).cloned().unwrap_or_default();
        let mut vars: Vars = service_env.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect();
        let Value::Seq(items) = files.value else { continue };
        for (i, f) in items.iter().enumerate() {
            let Some(fm) = f.as_map() else { continue };
            let Some(file) = fm.get("path").and_then(Node::as_str) else { continue };
            let required = fm.get("required").map(|r| r.value != Value::Bool(false)).unwrap_or(true);
            let content = match std::fs::read_to_string(file) {
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    if required {
                        diags.push(Diagnostic::error("env-file-not-found", format!("env_file {file} introuvable")).at(&path.index(i), f.span));
                    }
                    continue;
                }
                Err(e) => {
                    diags.push(Diagnostic::error("env-file-unreadable", format!("env_file {file} : {e}")).at(&path.index(i), f.span));
                    continue;
                }
            };
            let lookup = |k: &str| project_env.get(k).cloned().or_else(|| service_env.get(k).and_then(Node::as_str).map(str::to_string));
            if let Err(e) = dotenv::parse(&content, &mut vars, &lookup) {
                diags.push(Diagnostic::error("env-file-invalid", format!("env_file {file} : {e}")).at(&path.index(i), f.span));
            }
        }
        // `ToMappingWithEquals().OverrideBy(service.Environment)` : le
        // fichier d'abord, le service par-dessus (y compris ses null).
        let mut merged: IndexMap<String, Node> = vars.into_iter().map(|(k, v)| (k, Node::new(span, Value::Str(v)))).collect();
        for (k, v) in service_env {
            merged.insert(k, v);
        }
        merged.sort_keys();
        s.insert("environment".into(), Node::new(span, Value::Map(merged)));
    }
}
