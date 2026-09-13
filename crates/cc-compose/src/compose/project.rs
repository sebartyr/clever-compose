//! Chargement d'un projet : découverte des fichiers, puis la chaîne de
//! normalisation, étape par étape, chacune ajoutée quand le corpus l'exige.

use std::path::{Path as FsPath, PathBuf};

use indexmap::IndexMap;

use super::node::{Node, Path, Value};
use super::{merge, normalize, transform, typed, yaml};
use crate::diagnostic::{Diagnostic, Diagnostics, Files};

/// Noms cherchés dans le répertoire du projet, dans l'ordre de compose-go
/// (`loader.DefaultFileNames`), et leurs overrides.
const DEFAULT_FILES: &[&str] = &["compose.yaml", "compose.yml", "docker-compose.yaml", "docker-compose.yml"];
const OVERRIDE_FILES: &[&str] =
    &["compose.override.yaml", "compose.override.yml", "docker-compose.override.yaml", "docker-compose.override.yml"];

pub struct Options {
    pub project_dir: PathBuf,
    /// Fichiers passés explicitement (`-f`). Vide : découverte.
    pub files: Vec<PathBuf>,
    pub project_name: Option<String>,
}

pub struct Project {
    pub name: String,
    pub files: Files,
    /// Arbre normalisé, pas encore typé.
    pub tree: Node,
}

pub fn load(opts: &Options, diags: &mut Diagnostics) -> Option<Project> {
    let paths = match discover(opts) {
        Ok(p) => p,
        Err(d) => {
            diags.push(d);
            return None;
        }
    };

    let mut files = Files::default();
    let mut docs: Vec<Node> = Vec::new();
    for p in &paths {
        let display = p.strip_prefix(&opts.project_dir).unwrap_or(p).display().to_string();
        let idx = files.add(display.clone());
        let source = match std::fs::read_to_string(p) {
            Ok(s) => s,
            Err(e) => {
                diags.push(Diagnostic::error("file-unreadable", format!("{display} : {e}")));
                return None;
            }
        };
        match yaml::load(&source, idx) {
            Ok(n) => docs.push(n),
            Err(e) => {
                diags.push(Diagnostic::error("yaml-invalid", format!("{display} : {e}")));
                return None;
            }
        }
    }

    // Chaîne de compose-go (`loadYamlFile`), fichier par fichier : clés en
    // chaînes (fait au chargement), interpolation, include, extends, fusion
    // dans l'accumulateur, unicité, schéma, canonicalisation, OmitEmpty,
    // unicité. Les étapes absentes (interpolation, include, extends, schéma)
    // viendront avec les cas du corpus qui les exercent.
    let mut acc: Option<Node> = None;
    let mut first_doc: Option<Node> = None;
    for doc in docs {
        if doc.as_map().is_none() {
            diags.push(Diagnostic::error("root-not-mapping", "la racine du fichier doit être un mapping").at(&Path::root(), doc.span));
            return None;
        }
        if first_doc.is_none() {
            first_doc = Some(doc.clone());
        }
        let step = |r: Result<Node, Diagnostic>, diags: &mut Diagnostics| -> Option<Node> {
            match r {
                Ok(n) => Some(n),
                Err(d) => {
                    diags.push(d);
                    None
                }
            }
        };
        let merged = match acc.take() {
            None => doc,
            Some(a) => step(merge::merge(a, doc), diags)?,
        };
        let merged = step(merge::enforce_unicity(merged), diags)?;
        let merged = step(transform::canonical(merged), diags)?;
        let merged = transform::omit_empty(merged);
        acc = Some(step(merge::enforce_unicity(merged), diags)?);
    }
    let tree = acc?;
    let tree = match transform::set_defaults(tree) {
        Ok(t) => t,
        Err(d) => {
            diags.push(d);
            return None;
        }
    };
    let name = project_name(opts, first_doc.as_ref()?, diags)?;
    let env: IndexMap<String, String> = std::env::vars().collect();
    let tree = normalize::normalize(tree, &name, &env);
    let mut tree = match typed::apply(tree) {
        Ok(t) => t,
        Err(d) => {
            diags.push(d);
            return None;
        }
    };
    let span = tree.span;
    if let Some(m) = tree.as_map_mut() {
        m.shift_remove("name");
        m.shift_insert(0, "name".into(), Node::new(span, Value::Str(name.clone())));
    }
    Some(Project { name, files, tree })
}

fn discover(opts: &Options) -> Result<Vec<PathBuf>, Diagnostic> {
    if !opts.files.is_empty() {
        return Ok(opts.files.iter().map(|f| opts.project_dir.join(f)).collect());
    }
    let main = DEFAULT_FILES.iter().map(|f| opts.project_dir.join(f)).find(|p| p.is_file());
    let Some(main) = main else {
        return Err(Diagnostic::error(
            "no-compose-file",
            format!("aucun fichier compose dans {} (cherché : {})", opts.project_dir.display(), DEFAULT_FILES.join(", ")),
        ));
    };
    let mut out = vec![main];
    if let Some(over) = OVERRIDE_FILES.iter().map(|f| opts.project_dir.join(f)).find(|p| p.is_file()) {
        out.push(over);
    }
    Ok(out)
}

/// Nom du projet : option, puis `name:` du fichier, puis le nom du répertoire
/// normalisé comme compose-go (`NormalizeProjectName`).
fn project_name(opts: &Options, tree: &Node, diags: &mut Diagnostics) -> Option<String> {
    if let Some(n) = &opts.project_name {
        return Some(n.clone());
    }
    if let Some(n) = tree.as_map().and_then(|m| m.get("name")) {
        return match n.as_str() {
            Some(s) => Some(s.to_string()),
            None => {
                diags.push(Diagnostic::error("name-not-string", "`name` doit être une chaîne").at(&Path::root().child("name"), n.span));
                None
            }
        };
    }
    let dir = std::fs::canonicalize(&opts.project_dir).unwrap_or_else(|_| opts.project_dir.clone());
    let base = dir.file_name().and_then(|s| s.to_str()).unwrap_or("default");
    Some(normalize_project_name(base))
}

/// `NormalizeProjectName` de compose-go : minuscules, seuls `[a-z0-9_-]`
/// gardés, les caractères initiaux hors `[a-z0-9]` retirés.
pub fn normalize_project_name(s: &str) -> String {
    let lower = s.to_lowercase();
    let kept: String = lower.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
    kept.trim_start_matches(|c: char| !c.is_ascii_alphanumeric()).to_string()
}

pub fn project_dir_of(file: &FsPath) -> PathBuf {
    file.parent().map(FsPath::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}
