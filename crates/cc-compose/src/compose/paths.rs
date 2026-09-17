//! Résolution des chemins relatifs par rapport au répertoire du projet
//! (compose-go `paths.ResolveRelativePaths`).

use std::path::{Path as FsPath, PathBuf};

use super::node::{Node, Path, Value};
use crate::diagnostic::Diagnostic;

/// Résout les chemins relatifs à `working_dir`. `confine` : racine que
/// tout chemin doit rester sous (projet déposé sur un serveur, voir
/// [`Confine`]) ; `None` : comportement Compose, chemins absolus et `~`
/// admis.
pub fn resolve(node: Node, working_dir: &FsPath, confine: Option<&FsPath>) -> Result<Node, Diagnostic> {
    at(node, &Path::root(), working_dir, confine)
}

/// Un chemin de fichier résolu doit rester sous la racine du projet quand
/// celui-ci a été déposé (archive envoyée à un serveur, assets
/// rapatriés) : sinon un compose lirait n'importe quel fichier de la
/// machine du serveur (`env_file: /proc/self/environ`,
/// `secrets.file: ~/.config/…`). Vérification lexicale sur le chemin
/// nettoyé, et sur le chemin canonique s'il existe (liens symboliques).
pub fn check_confined(path: &str, confine: Option<&FsPath>, p: &Path, span: super::node::Span) -> Result<(), Diagnostic> {
    let Some(root) = confine else { return Ok(()) };
    let outside = |q: &FsPath| !q.starts_with(root);
    let cleaned = FsPath::new(path);
    let bad = outside(cleaned) || std::fs::canonicalize(cleaned).is_ok_and(|c| outside(&c));
    if bad {
        return Err(Diagnostic::error("path-outside-project", format!("`{path}` : hors du projet déposé ({}) ; un projet envoyé au serveur ne peut lire que ses propres fichiers", root.display())).at(p, span));
    }
    Ok(())
}

fn at(node: Node, p: &Path, wd: &FsPath, confine: Option<&FsPath>) -> Result<Node, Diagnostic> {
    let checked = |s: String, span: super::node::Span, p: &Path| -> Result<Node, Diagnostic> {
        check_confined(&s, confine, p, span)?;
        Ok(Node::new(span, Value::Str(s)))
    };
    if p.matches("services.*.build.context") || p.matches("services.*.build.additional_contexts.*") {
        return Ok(match node.value {
            Value::Str(ref s) if s.contains("://") || s.starts_with("service:") || is_remote_context(s) => node,
            Value::Str(s) => checked(abs_path(&s, wd), node.span, p)?,
            _ => node,
        });
    }
    if p.matches("services.*.build.dockerfile") && confine.is_some() {
        // Relatif au contexte (résolu à part) : vérifié depuis le
        // répertoire du projet, le contexte y étant déjà confiné.
        if let Value::Str(ref s) = node.value {
            check_confined(&join_clean(wd, s), confine, p, node.span)?;
        }
        return Ok(node);
    }
    if p.matches("services.*.build.ssh.*") || p.matches("configs.*.file") || p.matches("secrets.*.file") {
        return Ok(match node.value {
            Value::Str(s) => checked(maybe_unix_path(&s, wd), node.span, p)?,
            _ => node,
        });
    }
    if p.matches("services.*.env_file.*.path") || p.matches("services.*.label_file.*") || p.matches("include.path") || p.matches("include.project_directory") || p.matches("include.env_file") || p.matches("services.*.develop.watch.*.path") {
        return Ok(match node.value {
            Value::Str(s) => checked(abs_path(&s, wd), node.span, p)?,
            Value::Seq(items) => {
                let mut out = Vec::with_capacity(items.len());
                for (i, item) in items.into_iter().enumerate() {
                    out.push(match item.value {
                        Value::Str(s) => checked(abs_path(&s, wd), item.span, &p.index(i))?,
                        _ => item,
                    });
                }
                Node::new(node.span, Value::Seq(out))
            }
            _ => node,
        });
    }
    if p.matches("services.*.extends.file") {
        return Ok(match node.value {
            Value::Str(ref s) if is_remote_context(s) => node,
            Value::Str(s) => checked(abs_path(&s, wd), node.span, p)?,
            _ => node,
        });
    }
    if p.matches("services.*.volumes.*") {
        let Value::Map(mut m) = node.value else {
            return Ok(node);
        };
        if m.get("type").and_then(Node::as_str) == Some("bind") {
            let Some(src) = m.get("source").and_then(Node::as_str).map(str::to_string) else {
                return Err(Diagnostic::error("bind-missing-source", "montage de type `bind` sans `source`").at(p, node.span));
            };
            let resolved = maybe_unix_path(&src, wd);
            check_confined(&resolved, confine, &p.child("source"), node.span)?;
            m.insert("source".into(), Node::new(node.span, Value::Str(resolved)));
        }
        return Ok(Node { span: node.span, tag: node.tag, value: Value::Map(m) });
    }
    if p.matches("volumes.*") {
        let Value::Map(mut m) = node.value else {
            return Ok(node);
        };
        if m.get("driver").and_then(Node::as_str) == Some("local") {
            if let Some(opts) = m.get_mut("driver_opts").and_then(Node::as_map_mut) {
                if opts.get("o").and_then(Node::as_str) == Some("bind") {
                    if let Some(dev) = opts.get("device").and_then(Node::as_str).map(str::to_string) {
                        let span = opts["device"].span;
                        let resolved = maybe_unix_path(&dev, wd);
                        check_confined(&resolved, confine, &p.child("driver_opts").child("device"), span)?;
                        opts.insert("device".into(), Node::new(span, Value::Str(resolved)));
                    }
                }
            }
        }
        return Ok(Node { span: node.span, tag: node.tag, value: Value::Map(m) });
    }
    Ok(match node.value {
        Value::Map(m) => {
            let mut out = indexmap::IndexMap::with_capacity(m.len());
            for (k, v) in m {
                let t = at(v, &p.child(&k), wd, confine)?;
                out.insert(k, t);
            }
            Node { span: node.span, tag: node.tag, value: Value::Map(out) }
        }
        Value::Seq(s) => {
            let mut out = Vec::with_capacity(s.len());
            for (i, v) in s.into_iter().enumerate() {
                out.push(at(v, &p.index(i), wd, confine)?);
            }
            Node { span: node.span, tag: node.tag, value: Value::Seq(out) }
        }
        _ => node,
    })
}

fn is_remote_context(s: &str) -> bool {
    ["https://", "http://", "git://", "ssh://", "github.com/", "git@"].iter().any(|p| s.starts_with(p))
}

pub fn expand_user(p: &str) -> String {
    match p.strip_prefix('~') {
        Some(rest) => match std::env::var_os("HOME") {
            Some(home) => join_clean(FsPath::new(&home), rest.trim_start_matches('/')),
            None => p.to_string(),
        },
        None => p.to_string(),
    }
}

/// `filepath.Join(wd, p)` : jointure puis nettoyage.
fn join_clean(base: &FsPath, p: &str) -> String {
    let joined: PathBuf = if p.is_empty() { base.to_path_buf() } else { base.join(p) };
    super::transform::clean_path(&joined.to_string_lossy())
}

pub fn abs_path(p: &str, wd: &FsPath) -> String {
    let p = expand_user(p);
    if p.starts_with('/') || p.is_empty() {
        return p;
    }
    join_clean(wd, &p)
}

fn maybe_unix_path(p: &str, wd: &FsPath) -> String {
    let p = expand_user(p);
    if p.starts_with('/') || is_windows_abs(&p) {
        return p;
    }
    join_clean(wd, &p)
}

fn is_windows_abs(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}
