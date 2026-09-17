//! Chargement d'un modèle Compose : port de `loader/loader.go`,
//! `loader/extends.go`, `loader/include.go` et `loader/reset.go`.
//!
//! Par fichier : interpolation, `include`, application des `!reset` et
//! `!override` sur l'accumulé, `extends`, fusion, unicité, canonicalisation,
//! `OmitEmpty`, unicité. Une fois tous les fichiers fusionnés : valeurs par
//! défaut, chemins relatifs, environnement.

use std::collections::HashMap;
use std::path::{Path as FsPath, PathBuf};

use indexmap::IndexMap;

use super::dotenv::{self, Vars};
use super::node::{Node, Path, Value};
use super::{interpolate, merge, paths, schema, transform, yaml};
use crate::diagnostic::{Diagnostic, Files};

pub struct Config {
    pub working_dir: PathBuf,
    pub files: Vec<PathBuf>,
    pub env: Vars,
    /// Racine sous laquelle tout fichier lu doit rester (projet déposé),
    /// voir `paths::check_confined`.
    pub confine: Option<PathBuf>,
}

pub struct Loader<'a> {
    pub files: &'a mut Files,
    /// Répertoire du projet, pour afficher les fichiers relativement.
    root: PathBuf,
    /// Bases `extends` déjà chargées, par fichier.
    extends_cache: HashMap<PathBuf, IndexMap<String, Node>>,
}

type LoadResult<T> = Result<T, Diagnostic>;

impl<'a> Loader<'a> {
    pub fn new(files: &'a mut Files, root: PathBuf) -> Self {
        Loader { files, root, extends_cache: HashMap::new() }
    }

    /// `loadYamlModel` : tous les fichiers de `config`, fusionnés.
    pub fn load_model(&mut self, config: &Config, included: &[PathBuf]) -> LoadResult<Node> {
        let mut dict = Node::new(Default::default(), Value::Map(IndexMap::new()));
        for file in &config.files {
            dict = self.load_file(file, config, dict, included, &mut Vec::new())?;
        }
        let dict = transform::set_defaults(dict)?;
        paths::resolve(dict, &config.working_dir, config.confine.as_deref())
    }

    /// Un fichier lu par le chargeur (compose, include, extends, env) doit
    /// rester sous la racine d'un projet déposé.
    fn confined(config: &Config, file: &FsPath, p: &Path, span: super::node::Span) -> LoadResult<()> {
        paths::check_confined(&file.to_string_lossy(), config.confine.as_deref(), p, span)
    }

    /// `loadYamlFile` : un fichier fusionné dans `dict`.
    fn load_file(&mut self, file: &FsPath, config: &Config, dict: Node, included: &[PathBuf], tracker: &mut Vec<(PathBuf, String)>) -> LoadResult<Node> {
        let (cfg, _) = self.read_and_interpolate(file, config)?;
        let mut cfg = cfg;
        let resets = ResetPaths::collect(&mut cfg);

        let mut included = included.to_vec();
        included.push(file.to_path_buf());
        self.apply_include(&mut cfg, config, &included)?;

        let dict = resets.apply(dict);
        self.apply_extends(&mut cfg, file, config, tracker, &resets)?;

        let merged = merge::merge(dict, cfg)?;
        let merged = merge::enforce_unicity(merged)?;
        schema::validate(&merged)?;
        let merged = strip_version(merged);
        let merged = transform::canonical(merged)?;
        let merged = transform::omit_empty(merged);
        merge::enforce_unicity(merged)
    }

    fn read_and_interpolate(&mut self, file: &FsPath, config: &Config) -> LoadResult<(Node, usize)> {
        let env = &config.env;
        Self::confined(config, file, &Path::root(), Default::default())?;
        let display = file.strip_prefix(&self.root).unwrap_or(file).display().to_string();
        let idx = self.files.add(display.clone());
        let source = std::fs::read_to_string(file).map_err(|e| Diagnostic::error("file-unreadable", format!("{display} : {e}")))?;
        let raw = yaml::load(&source, idx).map_err(|e| Diagnostic::error("yaml-invalid", format!("{display} : {e}")))?;
        if raw.as_map().is_none() {
            return Err(Diagnostic::error("root-not-mapping", format!("{display} : la racine doit être un mapping")).at(&Path::root(), raw.span));
        }
        let lookup = |k: &str| env.get(k).cloned();
        let mut errs = Vec::new();
        let node = interpolate::interpolate(raw, &lookup, &mut errs);
        if let Some(first) = errs.into_iter().next() {
            return Err(first);
        }
        Ok((node, idx))
    }

    // --- include -----------------------------------------------------------

    fn apply_include(&mut self, model: &mut Node, config: &Config, included: &[PathBuf]) -> LoadResult<()> {
        let Some(include) = model.as_map_mut().and_then(|m| m.shift_remove("include")) else {
            return Ok(());
        };
        let Value::Seq(entries) = include.value else {
            return Err(Diagnostic::error("include-invalid", "`include` doit être une liste").at(&Path::root().child("include"), include.span));
        };
        for (i, entry) in entries.into_iter().enumerate() {
            let p = Path::root().child("include").index(i);
            let span = entry.span;
            let entry = match entry.value {
                Value::Str(s) => IndexMap::from([("path".to_string(), Node::new(span, Value::Str(s)))]),
                Value::Map(m) => m,
                _ => return Err(Diagnostic::error("include-invalid", "une entrée de `include` est un chemin ou un mapping").at(&p, span)),
            };
            let str_list = |key: &str| -> Vec<String> {
                match entry.get(key).map(|n| &n.value) {
                    Some(Value::Str(s)) => vec![s.clone()],
                    Some(Value::Seq(items)) => items.iter().filter_map(Node::as_str).map(str::to_string).collect(),
                    _ => Vec::new(),
                }
            };
            let rel_paths = str_list("path");
            if rel_paths.is_empty() {
                return Err(Diagnostic::error("include-invalid", "`include` : `path` manquant").at(&p, span));
            }
            let abs: Vec<PathBuf> = rel_paths.iter().map(|r| abs_join(&config.working_dir, r)).collect();
            let project_directory = match entry.get("project_directory").and_then(Node::as_str) {
                Some(d) => abs_join(&config.working_dir, d),
                None => abs[0].parent().map(FsPath::to_path_buf).unwrap_or_else(|| config.working_dir.clone()),
            };
            if included.contains(&abs[0]) {
                return Err(Diagnostic::error(
                    "include-cycle",
                    format!("cycle d'include détecté :\n{}\n include {}", included[0].display(), included[1..].iter().chain(std::iter::once(&abs[0])).map(|f| f.display().to_string()).collect::<Vec<_>>().join("\n include ")),
                )
                .at(&p, span));
            }
            let env_files: Vec<PathBuf> = {
                let listed = str_list("env_file");
                if listed.is_empty() {
                    let f = project_directory.join(".env");
                    if f.is_file() { vec![f] } else { Vec::new() }
                } else {
                    listed.iter().filter(|f| *f != "/dev/null").map(|f| abs_join(&config.working_dir, f)).collect()
                }
            };
            let mut env = config.env.clone();
            let mut from_files = Vars::new();
            for f in &env_files {
                Self::confined(config, f, &p, span)?;
                let content = std::fs::read_to_string(f).map_err(|e| Diagnostic::error("env-file-unreadable", format!("{} : {e}", f.display())).at(&p, span))?;
                let lookup = |k: &str| env.get(k).cloned();
                dotenv::parse(&content, &mut from_files, &lookup).map_err(|e| Diagnostic::error("env-file-invalid", format!("{} : {e}", f.display())).at(&p, span))?;
            }
            for (k, v) in from_files {
                env.entry(k).or_insert(v);
            }
            let sub = Config { working_dir: project_directory, files: abs, env, confine: config.confine.clone() };
            let imported = self.load_model(&sub, included)?;
            import_resources(imported, model)?;
        }
        Ok(())
    }

    // --- extends -----------------------------------------------------------

    fn apply_extends(&mut self, cfg: &mut Node, file: &FsPath, config: &Config, tracker: &mut Vec<(PathBuf, String)>, resets: &ResetPaths) -> LoadResult<()> {
        let Some(services) = cfg.as_map().and_then(|m| m.get("services")) else {
            return Ok(());
        };
        if services.as_map().is_none() {
            return Err(Diagnostic::error("services-not-mapping", "`services` doit être un mapping").at(&Path::root().child("services"), services.span));
        }
        let names: Vec<String> = services.as_map().unwrap().keys().cloned().collect();
        let mut services: IndexMap<String, Node> = services.as_map().unwrap().clone();
        for name in names {
            let merged = self.apply_service_extends(&name, &mut services, file, config, tracker, resets)?;
            if let Some(m) = merged {
                services.insert(name, m);
            }
        }
        let span = cfg.span;
        cfg.as_map_mut().unwrap().insert("services".into(), Node::new(span, Value::Map(services)));
        Ok(())
    }

    fn apply_service_extends(&mut self, name: &str, services: &mut IndexMap<String, Node>, file: &FsPath, config: &Config, tracker: &mut Vec<(PathBuf, String)>, resets: &ResetPaths) -> LoadResult<Option<Node>> {
        let Some(service) = services.get(name).cloned() else {
            return Ok(None);
        };
        if service.is_null() {
            return Ok(None);
        }
        let p = Path::root().child("services").child(name);
        let Some(sm) = service.as_map() else {
            return Err(Diagnostic::error("service-not-mapping", format!("services.{name} doit être un mapping")).at(&p, service.span));
        };
        let Some(extends) = sm.get("extends") else {
            return Ok(Some(service));
        };
        let (reference, ext_file) = match &extends.value {
            Value::Map(em) => {
                let Some(r) = em.get("service").and_then(Node::as_str) else {
                    return Err(Diagnostic::error("extends-invalid", format!("extends.{name}.service est requis")).at(&p.child("extends"), extends.span));
                };
                (r.to_string(), em.get("file").and_then(Node::as_str).map(str::to_string))
            }
            Value::Str(s) => (s.clone(), None),
            _ => return Err(Diagnostic::error("extends-invalid", "`extends` est un nom de service ou un mapping").at(&p.child("extends"), extends.span)),
        };

        let (base_file, mut base_services): (PathBuf, IndexMap<String, Node>) = match ext_file {
            Some(f) => {
                let abs = abs_join(&config.working_dir, &f);
                let bases = self.extends_base(&abs, config, name, &reference, file, extends.span, &p)?;
                (abs, bases)
            }
            None => {
                if !services.contains_key(&reference) {
                    return Err(Diagnostic::error("extends-unknown-service", format!("impossible d'étendre le service `{name}` dans {} : service `{reference}` introuvable", file.display())).at(&p.child("extends"), extends.span));
                }
                (file.to_path_buf(), services.clone())
            }
        };

        let key = (base_file.clone(), name.to_string());
        if tracker.contains(&key) {
            let chain: Vec<String> = tracker.iter().map(|(f, s)| format!("{}: {s}", f.display())).collect();
            return Err(Diagnostic::error("extends-cycle", format!("cycle d'extends détecté : {} -> {}: {name}", chain.join(" -> "), base_file.display())).at(&p.child("extends"), extends.span));
        }
        tracker.push(key);
        let base = self.apply_service_extends(&reference, &mut base_services, &base_file, config, tracker, resets)?;
        tracker.pop();

        let Some(base) = base else {
            return Ok(Some(service));
        };
        // Les !reset/!override du service étendant s'appliquent à la base.
        let base = resets.apply_to_service(name, base);
        let mut merged = merge::merge_service(base, service)?;
        if let Some(m) = merged.as_map_mut() {
            m.shift_remove("extends");
        }
        Ok(Some(merged))
    }

    /// `getExtendsBaseFromFile` + `loadExtendsBase` : les services bruts
    /// (interpolés, chemins résolus, ni canonicalisés ni normalisés) du
    /// fichier de base.
    #[allow(clippy::too_many_arguments)]
    fn extends_base(&mut self, abs: &FsPath, config: &Config, name: &str, reference: &str, from: &FsPath, span: super::node::Span, p: &Path) -> LoadResult<IndexMap<String, Node>> {
        if !self.extends_cache.contains_key(abs) {
            // Comme `loadYamlFile` avec include, extends, validation et
            // défauts sautés : canonicalisation, OmitEmpty, unicité, puis
            // chemins relatifs au répertoire du fichier de base.
            let (mut node, _) = self.read_and_interpolate(abs, config)?;
            let _ = ResetPaths::collect(&mut node);
            let node = merge::enforce_unicity(node)?;
            let node = transform::canonical(node)?;
            let node = transform::omit_empty(node);
            let node = merge::enforce_unicity(node)?;
            let wd = abs.parent().map(FsPath::to_path_buf).unwrap_or_else(|| config.working_dir.clone());
            let node = paths::resolve(node, &wd, config.confine.as_deref())?;
            let Some(services) = node.as_map().and_then(|m| m.get("services")).and_then(Node::as_map).cloned() else {
                return Err(Diagnostic::error("extends-no-services", format!("impossible d'étendre le service `{name}` dans {} : pas de section services", abs.display())).at(p, span));
            };
            self.extends_cache.insert(abs.to_path_buf(), services);
        }
        let services = self.extends_cache[abs].clone();
        if !services.contains_key(reference) {
            return Err(Diagnostic::error(
                "extends-unknown-service",
                format!("impossible d'étendre le service `{name}` dans {} : service `{reference}` introuvable dans {}", from.display(), abs.display()),
            )
            .at(&p.child("extends"), span));
        }
        Ok(services)
    }
}

/// `version` est obsolète : ignorée (compose-go avertit puis la retire).
fn strip_version(mut node: Node) -> Node {
    if let Some(m) = node.as_map_mut() {
        m.shift_remove("version");
    }
    node
}

fn abs_join(base: &FsPath, p: &str) -> PathBuf {
    let p = paths::expand_user(p);
    if p.starts_with('/') {
        PathBuf::from(p)
    } else {
        PathBuf::from(transform::clean_path(&base.join(p).to_string_lossy()))
    }
}

/// `importResources` : les ressources d'un modèle inclus entrent dans le
/// modèle courant ; un conflit se fusionne comme un override, une
/// définition identique (losange d'includes) est ignorée.
fn import_resources(imported: Node, model: &mut Node) -> LoadResult<()> {
    let Value::Map(mut from) = imported.value else { return Ok(()) };
    let span = model.span;
    let target = model.as_map_mut().unwrap();
    for key in ["services", "volumes", "networks", "secrets", "configs", "models"] {
        let Some(Value::Map(src)) = from.shift_remove(key).map(|n| n.value) else { continue };
        let mut to: IndexMap<String, Node> = target.get(key).and_then(Node::as_map).cloned().unwrap_or_default();
        for (name, a) in src {
            match to.shift_remove(&name) {
                None => {
                    to.insert(name, a);
                }
                Some(conflict) if merge::same_value(&a, &conflict) => {
                    to.insert(name, conflict);
                }
                Some(conflict) => {
                    let merged = merge::merge_at_path(a, conflict, &Path::root().child(key).child(&name))?;
                    to.insert(name, merged);
                }
            }
        }
        target.insert(key.into(), Node::new(span, Value::Map(to)));
    }
    Ok(())
}

/// Chemins des nœuds `!reset` (valeur remise à null) et `!override` d'un
/// fichier ; appliqués à l'accumulé, ils y suppriment la clé pour que la
/// valeur du fichier remplace sans fusion.
pub struct ResetPaths(Vec<Path>);

impl ResetPaths {
    pub fn collect(node: &mut Node) -> Self {
        let mut out = Vec::new();
        if collect_resets(node, &Path::root(), &mut out) {
            node.value = Value::Map(IndexMap::new());
        }
        ResetPaths(out)
    }

    pub fn apply(&self, node: Node) -> Node {
        if self.0.is_empty() {
            return node;
        }
        apply_resets(node, &Path::root(), &self.0)
    }

    /// Pour `extends` : les chemins sous `services.<name>` s'appliquent à
    /// la base du service.
    fn apply_to_service(&self, name: &str, base: Node) -> Node {
        let prefix = Path::root().child("services").child(name);
        let rel: Vec<Path> = self
            .0
            .iter()
            .filter_map(|p| p.segments().strip_prefix(prefix.segments()).map(|rest| Path::from_segments(rest.to_vec())))
            .collect();
        if rel.is_empty() {
            return base;
        }
        apply_resets(base, &Path::root(), &rel)
    }
}

/// Renvoie `true` si le nœud est `!reset` : son chemin est enregistré et le
/// parent le retire (compose-go `resolveContainer`).
fn collect_resets(node: &mut Node, p: &Path, out: &mut Vec<Path>) -> bool {
    match node.tag.as_deref() {
        Some("!reset") => {
            out.push(p.clone());
            return true;
        }
        Some("!override") => {
            out.push(p.clone());
            node.tag = None;
        }
        _ => {}
    }
    match &mut node.value {
        Value::Map(m) => {
            let mut removed = Vec::new();
            for (k, v) in m.iter_mut() {
                if collect_resets(v, &p.child(k), out) {
                    removed.push(k.clone());
                }
            }
            for k in removed {
                m.shift_remove(&k);
            }
        }
        Value::Seq(s) => {
            let mut keep = Vec::with_capacity(s.len());
            for (i, v) in s.iter_mut().enumerate() {
                keep.push(!collect_resets(v, &p.index(i), out));
            }
            let mut it = keep.into_iter();
            s.retain(|_| it.next().unwrap_or(true));
        }
        _ => {}
    }
    false
}

fn apply_resets(node: Node, p: &Path, resets: &[Path]) -> Node {
    match node.value {
        Value::Map(m) => {
            let mut out = IndexMap::with_capacity(m.len());
            for (k, v) in m {
                let next = p.child(&k);
                if resets.iter().any(|r| next.matches(&r.to_string())) {
                    continue;
                }
                out.insert(k, apply_resets(v, &next, resets));
            }
            Node { span: node.span, tag: node.tag, value: Value::Map(out) }
        }
        Value::Seq(s) => Node { span: node.span, tag: node.tag, value: Value::Seq(s.into_iter().enumerate().map(|(i, v)| apply_resets(v, &p.index(i), resets)).collect()) },
        _ => node,
    }
}
