//! Résolution des chemins relatifs par rapport au répertoire du projet
//! (compose-go `paths.ResolveRelativePaths`).

use std::path::{Path as FsPath, PathBuf};

use super::node::{Node, Path, Value};
use crate::diagnostic::Diagnostic;

pub fn resolve(node: Node, working_dir: &FsPath) -> Result<Node, Diagnostic> {
    at(node, &Path::root(), working_dir)
}

fn at(node: Node, p: &Path, wd: &FsPath) -> Result<Node, Diagnostic> {
    if p.matches("services.*.build.context") || p.matches("services.*.build.additional_contexts.*") {
        return Ok(match node.value {
            Value::Str(ref s) if s.contains("://") || s.starts_with("service:") || is_remote_context(s) => node,
            Value::Str(s) => Node::new(node.span, Value::Str(abs_path(&s, wd))),
            _ => node,
        });
    }
    if p.matches("services.*.build.ssh.*") || p.matches("configs.*.file") || p.matches("secrets.*.file") {
        return Ok(match node.value {
            Value::Str(s) => Node::new(node.span, Value::Str(maybe_unix_path(&s, wd))),
            _ => node,
        });
    }
    if p.matches("services.*.env_file.*.path") || p.matches("services.*.label_file.*") || p.matches("include.path") || p.matches("include.project_directory") || p.matches("include.env_file") || p.matches("services.*.develop.watch.*.path") {
        return Ok(match node.value {
            Value::Str(s) => Node::new(node.span, Value::Str(abs_path(&s, wd))),
            Value::Seq(items) => Node::new(node.span, Value::Seq(items.into_iter().map(|i| match i.value {
                Value::Str(s) => Node::new(i.span, Value::Str(abs_path(&s, wd))),
                _ => i,
            }).collect())),
            _ => node,
        });
    }
    if p.matches("services.*.extends.file") {
        return Ok(match node.value {
            Value::Str(ref s) if is_remote_context(s) => node,
            Value::Str(s) => Node::new(node.span, Value::Str(abs_path(&s, wd))),
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
            m.insert("source".into(), Node::new(node.span, Value::Str(maybe_unix_path(&src, wd))));
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
                        opts.insert("device".into(), Node::new(span, Value::Str(maybe_unix_path(&dev, wd))));
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
                let t = at(v, &p.child(&k), wd)?;
                out.insert(k, t);
            }
            Node { span: node.span, tag: node.tag, value: Value::Map(out) }
        }
        Value::Seq(s) => {
            let mut out = Vec::with_capacity(s.len());
            for (i, v) in s.into_iter().enumerate() {
                out.push(at(v, &p.index(i), wd)?);
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
