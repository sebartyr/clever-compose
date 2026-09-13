//! `loader.Normalize` : réseau par défaut, `depends_on` implicites, cibles
//! de volumes nettoyées, noms des ressources de premier niveau.

use indexmap::IndexMap;

use super::node::{Node, Value};
use super::transform::clean_path;

pub fn normalize(mut tree: Node, project_name: &str, env: &IndexMap<String, String>) -> Node {
    normalize_networks(&mut tree);
    let span = tree.span;
    if let Some(services) = tree.as_map_mut().and_then(|m| m.get_mut("services")).and_then(Node::as_map_mut) {
        for (_, service) in services.iter_mut() {
            let Some(s) = service.as_map_mut() else { continue };
            if s.get("pull_policy").and_then(Node::as_str) == Some("if_not_present") {
                s.insert("pull_policy".into(), Node::new(span, Value::Str("missing".into())));
            }
            if let Some(build) = s.get_mut("build").and_then(Node::as_map_mut) {
                if build.get("context").is_none_or(Node::is_null) {
                    build.insert("context".into(), Node::new(span, Value::Str(".".into())));
                }
                if build.get("dockerfile").is_none_or(Node::is_null) && build.get("dockerfile_inline").is_none_or(Node::is_null) {
                    build.insert("dockerfile".into(), Node::new(span, Value::Str("Dockerfile".into())));
                }
                if let Some(args) = build.get("args").cloned() {
                    build.insert("args".into(), resolve_env(args, env, false));
                }
            }
            if let Some(e) = s.get("environment").cloned() {
                s.insert("environment".into(), resolve_env(e, env, true));
            }

            let mut depends: IndexMap<String, Node> = s.get("depends_on").and_then(Node::as_map).cloned().unwrap_or_default();
            let implicit = |restart: bool, span| {
                Node::new(
                    span,
                    Value::Map(IndexMap::from([
                        ("condition".to_string(), Node::new(span, Value::Str("service_started".into()))),
                        ("restart".to_string(), Node::new(span, Value::Bool(restart))),
                        ("required".to_string(), Node::new(span, Value::Bool(true))),
                    ])),
                )
            };
            if let Some(links) = s.get("links").and_then(Node::as_seq) {
                for l in links {
                    if let Some(link) = l.as_str() {
                        let parts: Vec<&str> = link.split(':').collect();
                        let name = if parts.len() == 2 { parts[0] } else { link };
                        depends.entry(name.to_string()).or_insert_with(|| implicit(true, l.span));
                    }
                }
            }
            for ns in ["network_mode", "ipc", "pid", "uts", "cgroup"] {
                if let Some(shared) = s.get(ns).and_then(Node::as_str).and_then(|r| r.strip_prefix("service:")) {
                    let shared = shared.to_string();
                    depends.entry(shared).or_insert_with(|| implicit(true, span));
                }
            }
            if let Some(vols) = s.get_mut("volumes").and_then(|v| match &mut v.value {
                Value::Seq(items) => Some(items),
                _ => None,
            }) {
                for vol in vols.iter_mut() {
                    if let Some(vm) = vol.as_map_mut() {
                        if let Some(t) = vm.get("target").and_then(Node::as_str).map(clean_path) {
                            vm.insert("target".into(), Node::new(span, Value::Str(t)));
                        }
                    }
                }
            }
            if let Some(from) = s.get("volumes_from").and_then(Node::as_seq) {
                for v in from {
                    if let Some(vol) = v.as_str() {
                        if !vol.starts_with("container:") {
                            let name = vol.split(':').next().unwrap_or(vol).to_string();
                            depends.entry(name).or_insert_with(|| implicit(false, v.span));
                        }
                    }
                }
            }
            if !depends.is_empty() {
                s.insert("depends_on".into(), Node::new(span, Value::Map(depends)));
            }
            inherit_pre_start_image(s, span);
        }
    }
    set_name_from_key(&mut tree, project_name);
    tree
}

fn inherit_pre_start_image(s: &mut IndexMap<String, Node>, span: super::node::Span) {
    let Some(image) = s.get("image").and_then(Node::as_str).filter(|i| !i.is_empty()).map(str::to_string) else { return };
    if let Some(hooks) = s.get_mut("pre_start").and_then(|h| match &mut h.value {
        Value::Seq(items) => Some(items),
        _ => None,
    }) {
        for h in hooks.iter_mut() {
            if let Some(hm) = h.as_map_mut() {
                hm.entry("image".into()).or_insert_with(|| Node::new(span, Value::Str(image.clone())));
            }
        }
    }
}

/// `resolve` : les variables sans valeur (`FOO` ou `FOO:`) prennent celle
/// de l'environnement de l'hôte ; sans valeur non plus, gardées à null si
/// `keep_empty`, sinon retirées.
fn resolve_env(node: Node, env: &IndexMap<String, String>, keep_empty: bool) -> Node {
    let span = node.span;
    match node.value {
        Value::Seq(items) => {
            let mut out = Vec::new();
            for i in items {
                match i.value {
                    Value::Str(ref s) if !s.contains('=') => {
                        if let Some(v) = env.get(s) {
                            out.push(Node::new(i.span, Value::Str(format!("{s}={v}"))));
                        } else if keep_empty {
                            out.push(i);
                        }
                    }
                    _ => out.push(i),
                }
            }
            Node::new(span, Value::Seq(out))
        }
        Value::Map(m) => {
            let mut out = IndexMap::new();
            for (k, v) in m {
                if !v.is_null() {
                    out.insert(k, v);
                } else if let Some(s) = env.get(&k) {
                    out.insert(k, Node::new(v.span, Value::Str(s.clone())));
                } else if keep_empty {
                    out.insert(k, v);
                }
            }
            Node::new(span, Value::Map(out))
        }
        _ => node,
    }
}

fn normalize_networks(tree: &mut Node) {
    let span = tree.span;
    let mut uses_default = false;
    let Some(root) = tree.as_map_mut() else { return };
    if let Some(services) = root.get_mut("services").and_then(Node::as_map_mut) {
        for (_, service) in services.iter_mut() {
            let Some(s) = service.as_map_mut() else { continue };
            if s.contains_key("provider") || s.contains_key("network_mode") {
                continue;
            }
            let default = || Node::new(span, Value::Map(IndexMap::from([("default".to_string(), Node::new(span, Value::Null))])));
            match s.get("networks").and_then(Node::as_map) {
                None => {
                    s.insert("networks".into(), default());
                    uses_default = true;
                }
                Some(net) if net.is_empty() => {
                    s.insert("networks".into(), default());
                    uses_default = true;
                }
                Some(net) if net.contains_key("default") => uses_default = true,
                Some(_) => {}
            }
        }
    }
    let mut networks: IndexMap<String, Node> = root.get("networks").and_then(Node::as_map).cloned().unwrap_or_default();
    if uses_default && !networks.contains_key("default") {
        networks.insert("default".into(), Node::new(span, Value::Null));
    }
    if !networks.is_empty() {
        root.insert("networks".into(), Node::new(span, Value::Map(networks)));
    }
}

/// `setNameFromKey` : `name` par défaut `<projet>_<clé>`, ou la clé seule
/// pour une ressource externe.
fn set_name_from_key(tree: &mut Node, project_name: &str) {
    let span = tree.span;
    let Some(root) = tree.as_map_mut() else { return };
    for kind in ["networks", "volumes", "configs", "secrets"] {
        let Some(top) = root.get_mut(kind).and_then(Node::as_map_mut) else { continue };
        for (key, resource) in top.iter_mut() {
            let mut m = resource.as_map().cloned().unwrap_or_default();
            if m.get("name").is_none_or(Node::is_null) {
                let external = m.get("external").map(super::merge::go_sprint).is_some_and(|s| matches!(s.as_str(), "1" | "t" | "T" | "TRUE" | "true" | "True"));
                let name = if external { key.clone() } else { format!("{project_name}_{key}") };
                m.insert("name".into(), Node::new(span, Value::Str(name)));
            }
            *resource = Node::new(resource.span, Value::Map(m));
        }
    }
}
