//! Chargement d'un projet : découverte des fichiers, environnement, puis la
//! chaîne du [`loader`], la normalisation, le typage, les `env_file`, les
//! profils.

use std::path::{Path as FsPath, PathBuf};

use super::dotenv::{self, Vars};
use super::loader::{Config, Loader};
use super::node::{Node, Path, Value};
use super::{envfile, normalize, typed};
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
    /// Fichiers d'environnement (`--env-file`). Vide : `.env` du projet.
    pub env_files: Vec<PathBuf>,
    /// Profils actifs (`--profile`, `COMPOSE_PROFILES`).
    pub profiles: Vec<String>,
    /// Projet déposé (archive envoyée à un serveur, assets rapatriés) :
    /// aucun fichier lu hors de `project_dir` (`path-outside-project`).
    pub confine: bool,
}

pub struct Project {
    pub name: String,
    pub files: Files,
    /// Arbre normalisé, pas encore typé.
    pub tree: Node,
}

pub fn load(opts: &Options, diags: &mut Diagnostics) -> Option<Project> {
    match load_inner(opts) {
        Ok((project, warnings)) => {
            diags.items.extend(warnings);
            diags.files = project.files.clone();
            Some(project)
        }
        Err((d, files)) => {
            diags.files = files;
            diags.push(d);
            None
        }
    }
}

#[allow(clippy::result_large_err)] // l'échec porte la table des fichiers, pour rendre la position
fn load_inner(opts: &Options) -> Result<(Project, Vec<Diagnostic>), (Diagnostic, Files)> {
    let mut files = Files::default();
    let fail = |d: Diagnostic, files: &Files| (d, files.clone());

    let paths = discover(opts).map_err(|d| fail(d, &files))?;
    let working_dir = std::fs::canonicalize(&opts.project_dir).unwrap_or_else(|_| opts.project_dir.clone());
    let env = project_env(opts).map_err(|d| fail(d, &files))?;
    let profiles: Vec<String> = if opts.profiles.is_empty() {
        env.get("COMPOSE_PROFILES").map(|p| p.split(',').map(str::to_string).collect()).unwrap_or_default()
    } else {
        opts.profiles.clone()
    };

    let config = Config { working_dir: working_dir.clone(), files: paths, env: env.clone(), confine: opts.confine.then(|| working_dir.clone()) };
    let tree = {
        let mut loader = Loader::new(&mut files, working_dir.clone());
        loader.load_model(&config, &[])
    }
    .map_err(|d| fail(d, &files))?;
    if tree.as_map().is_some_and(|m| m.is_empty()) {
        return Err(fail(Diagnostic::error("empty-compose-file", "fichier compose vide"), &files));
    }

    let name = project_name(opts, &tree, &env).map_err(|d| fail(d, &files))?;
    let tree = normalize::normalize(tree, &name, &env);
    let mut tree = typed::apply(tree).map_err(|d| fail(d, &files))?;
    let mut errs = Vec::new();
    envfile::resolve(&mut tree, &env, &mut errs);
    if let Some(d) = errs.into_iter().next() {
        return Err(fail(d, &files));
    }
    let mut warnings = Vec::new();
    apply_profiles(&mut tree, &profiles, &mut warnings).map_err(|d| fail(d, &files))?;
    typed::prune_unused(&mut tree);
    let mut tree = typed::omit_zero(tree);
    let span = tree.span;
    if let Some(m) = tree.as_map_mut() {
        m.shift_remove("name");
        m.shift_insert(0, "name".into(), Node::new(span, Value::Str(name.clone())));
    }
    // Cohérence (compose-go `checkConsistency`), sur le modèle typé.
    match super::model::Project::from_tree(&tree) {
        Ok(typed) => super::consistency::check(&typed).map_err(|d| fail(d, &files))?,
        Err(e) => return Err(fail(Diagnostic::error("model", format!("modèle Compose inattendu : {e}")), &files)),
    }
    Ok((Project { name, files, tree }, warnings))
}

/// Environnement du projet : le shell, puis `.env` sans écraser
/// (compose-go `WithOsEnv` puis `WithDotEnv`).
fn project_env(opts: &Options) -> Result<Vars, Diagnostic> {
    let mut env: Vars = std::env::vars().collect();
    let files: Vec<PathBuf> = if opts.env_files.is_empty() {
        let default = opts.project_dir.join(".env");
        if default.is_file() { vec![default] } else { Vec::new() }
    } else {
        opts.env_files.iter().map(|f| opts.project_dir.join(f)).collect()
    };
    let mut from_files = Vars::new();
    for f in files {
        let content = std::fs::read_to_string(&f).map_err(|e| Diagnostic::error("env-file-unreadable", format!("{} : {e}", f.display())))?;
        let lookup = |k: &str| env.get(k).cloned();
        dotenv::parse(&content, &mut from_files, &lookup).map_err(|e| Diagnostic::error("env-file-invalid", format!("{} : {e}", f.display())))?;
    }
    for (k, v) in from_files {
        env.entry(k).or_insert(v);
    }
    Ok(env)
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

/// Nom du projet : option, `COMPOSE_PROJECT_NAME`, `name:` du fichier, puis
/// le nom du répertoire, normalisé comme compose-go (`NormalizeProjectName`).
fn project_name(opts: &Options, tree: &Node, env: &Vars) -> Result<String, Diagnostic> {
    if let Some(n) = &opts.project_name {
        return Ok(n.clone());
    }
    if let Some(n) = env.get("COMPOSE_PROJECT_NAME").filter(|n| !n.is_empty()) {
        return Ok(n.clone());
    }
    if let Some(n) = tree.as_map().and_then(|m| m.get("name")) {
        return match n.as_str() {
            Some(s) => Ok(s.to_string()),
            None => Err(Diagnostic::error("name-not-string", "`name` doit être une chaîne").at(&Path::root().child("name"), n.span)),
        };
    }
    let dir = std::fs::canonicalize(&opts.project_dir).unwrap_or_else(|_| opts.project_dir.clone());
    let base = dir.file_name().and_then(|s| s.to_str()).unwrap_or("default");
    Ok(normalize_project_name(base))
}

/// `NormalizeProjectName` de compose-go : minuscules, seuls `[a-z0-9_-]`
/// gardés, les caractères initiaux hors `[a-z0-9]` retirés.
pub fn normalize_project_name(s: &str) -> String {
    let lower = s.to_lowercase();
    let kept: String = lower.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
    kept.trim_start_matches(|c: char| !c.is_ascii_alphanumeric()).to_string()
}

/// Profils : un service sans `profiles` est toujours actif ; un service
/// dont aucun profil n'est actif est retiré ; un `depends_on` vers un
/// service inactif est une erreur (CLAUDE.md §9.4).
fn apply_profiles(tree: &mut Node, active: &[String], warnings: &mut Vec<Diagnostic>) -> Result<(), Diagnostic> {
    let Some(services) = tree.as_map_mut().and_then(|m| m.get_mut("services")).and_then(Node::as_map_mut) else {
        return Ok(());
    };
    let enabled = |s: &Node| -> bool {
        match s.as_map().and_then(|m| m.get("profiles")).and_then(Node::as_seq) {
            None => true,
            Some(profiles) => profiles.is_empty() || active.iter().any(|a| a == "*") || profiles.iter().filter_map(Node::as_str).any(|p| active.iter().any(|a| a == p)),
        }
    };
    let disabled: Vec<String> = services.iter().filter(|(_, s)| !enabled(s)).map(|(k, _)| k.clone()).collect();
    for (name, s) in services.iter() {
        if disabled.contains(name) {
            continue;
        }
        if let Some(deps) = s.as_map().and_then(|m| m.get("depends_on")).and_then(Node::as_map) {
            for (dep, cfg) in deps {
                if disabled.contains(dep) {
                    let required = cfg.as_map().and_then(|m| m.get("required")).map(|r| r.value != Value::Bool(false)).unwrap_or(true);
                    let p = Path::root().child("services").child(name).child("depends_on").child(dep);
                    if required {
                        return Err(Diagnostic::error("depends-on-inactive-profile", format!("le service `{name}` dépend de `{dep}`, inactif (profil non activé)")).at(&p, cfg.span));
                    }
                    warnings.push(Diagnostic::warning("depends-on-inactive-profile", format!("le service `{name}` dépend de `{dep}`, inactif (profil non activé) ; dépendance optionnelle ignorée")).at(&p, cfg.span));
                }
            }
        }
    }
    for name in disabled {
        services.shift_remove(&name);
    }
    Ok(())
}

pub fn project_dir_of(file: &FsPath) -> PathBuf {
    file.parent().map(FsPath::to_path_buf).unwrap_or_else(|| PathBuf::from("."))
}
