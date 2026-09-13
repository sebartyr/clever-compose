//! Fusion des fichiers (overrides) et unicité des séquences.
//!
//! Port de `override/merge.go` et `override/uncity.go` de compose-go. Ce
//! n'est pas un merge JSON : chaque chemin a sa règle, et une règle fausse
//! donne un déploiement faux sans message (CLAUDE.md §9). Les règles sont
//! reprises une à une, avec leur chemin.

use indexmap::IndexMap;

use super::node::{Node, Path, Value};
use crate::diagnostic::Diagnostic;

/// Fusionne `other` (fichier suivant) dans `base` (accumulé). `base` gagne
/// pour les clés absentes de `other` ; sinon la règle du chemin décide.
pub fn merge(base: Node, other: Node) -> Result<Node, Diagnostic> {
    merge_at(base, other, &Path::root())
}

/// Fusion à un chemin donné (ressources importées par `include`).
pub fn merge_at_path(base: Node, other: Node, p: &Path) -> Result<Node, Diagnostic> {
    merge_at(base, other, p)
}

/// `override.ExtendService` : fusion d'un service sur sa base, avec les
/// règles de `services.*`.
pub fn merge_service(base: Node, other: Node) -> Result<Node, Diagnostic> {
    merge_at(base, other, &Path::root().child("services").child("x"))
}

fn merge_at(base: Node, other: Node, p: &Path) -> Result<Node, Diagnostic> {
    // `mergeSpecials` de compose-go, dans le même ordre logique.
    const OVERRIDE: &[&str] = &["services.*.command", "services.*.entrypoint", "services.*.healthcheck.test"];
    const TO_SEQUENCE: &[&str] = &[
        "networks.*.labels",
        "volumes.*.labels",
        "services.*.annotations",
        "services.*.build.args",
        "services.*.build.additional_contexts",
        "services.*.build.extra_hosts",
        "services.*.build.labels",
        "services.*.deploy.labels",
        "services.*.dns",
        "services.*.dns_opt",
        "services.*.dns_search",
        "services.*.env_file",
        "services.*.label_file",
        "services.*.environment",
        "services.*.extra_hosts",
        "services.*.labels",
        "services.*.volumes.*.volume.labels",
        "services.*.sysctls",
        "services.*.tmpfs",
    ];
    if OVERRIDE.iter().any(|pat| p.matches(pat)) {
        return Ok(other);
    }
    if TO_SEQUENCE.iter().any(|pat| p.matches(pat)) {
        let right = into_sequence(base);
        let left = into_sequence(other);
        return Ok(Node::new(right.span, Value::Seq(append_without_duplicates(seq_of(right), seq_of(left)))));
    }
    if p.matches("services.*.depends_on") {
        let defaults = [("condition", Value::Str("service_started".into())), ("required", Value::Bool(true))];
        return merge_as_mapping(base, other, &defaults, p);
    }
    if p.matches("services.*.networks") || p.matches("services.*.models") {
        return merge_as_mapping(base, other, &[], p);
    }
    if p.matches("services.*.build") {
        let to_build = |n: Node| -> IndexMap<String, Node> {
            match n.value {
                Value::Str(s) => IndexMap::from([("context".to_string(), Node::new(n.span, Value::Str(s)))]),
                Value::Map(m) => m,
                _ => IndexMap::new(),
            }
        };
        let span = base.span;
        return Ok(Node::new(span, Value::Map(merge_mappings(to_build(base), to_build(other), p)?)));
    }
    if p.matches("services.*.logging") {
        return merge_logging(base, other, p);
    }
    if p.matches("services.*.ulimits.*") {
        // Un mapping (soft/hard) remplace entièrement un scalaire, et
        // inversement : pas de fusion partielle.
        return Ok(other);
    }
    if p.matches("networks.*.ipam.config") {
        return merge_ipam_config(base, other);
    }

    if other.is_null() {
        return Ok(base);
    }
    match base.value {
        Value::Map(bm) => {
            let Value::Map(om) = other.value else {
                return Err(Diagnostic::error("merge-type-mismatch", format!("impossible de surcharger {p} : un mapping est attendu")).at(p, other.span));
            };
            Ok(Node { span: base.span, tag: base.tag, value: Value::Map(merge_mappings(bm, om, p)?) })
        }
        Value::Seq(bs) => {
            let Value::Seq(os) = other.value else {
                return Err(Diagnostic::error("merge-type-mismatch", format!("impossible de surcharger {p} : une liste est attendue")).at(p, other.span));
            };
            Ok(Node { span: base.span, tag: base.tag, value: Value::Seq(append_without_duplicates(bs, os)) })
        }
        _ => Ok(other),
    }
}

fn merge_mappings(mut base: IndexMap<String, Node>, other: IndexMap<String, Node>, p: &Path) -> Result<IndexMap<String, Node>, Diagnostic> {
    for (k, v) in other {
        match base.shift_remove(&k) {
            None => {
                base.insert(k, v);
            }
            Some(e) => {
                let merged = merge_at(e, v, &p.child(&k))?;
                base.insert(k, merged);
            }
        }
    }
    Ok(base)
}

fn seq_of(n: Node) -> Vec<Node> {
    match n.value {
        Value::Seq(s) => s,
        _ => Vec::new(),
    }
}

/// `convertIntoSequence` : un mapping devient `k=v` trié (une entrée par
/// valeur si la valeur est une liste, `k` seul si la valeur est nulle).
fn into_sequence(n: Node) -> Node {
    let span = n.span;
    match n.value {
        Value::Map(m) => {
            let mut seq: Vec<String> = Vec::new();
            for (k, v) in m {
                match v.value {
                    Value::Null => seq.push(k),
                    Value::Seq(items) => {
                        for i in items {
                            seq.push(format!("{k}={}", go_sprint(&i)));
                        }
                    }
                    _ => seq.push(format!("{k}={}", go_sprint(&v))),
                }
            }
            seq.sort();
            Node::new(span, Value::Seq(seq.into_iter().map(|s| Node::new(span, Value::Str(s))).collect()))
        }
        Value::Seq(_) => n,
        Value::Str(s) => Node::new(span, Value::Seq(vec![Node::new(span, Value::Str(s))])),
        _ => Node::new(span, Value::Seq(Vec::new())),
    }
}

/// `fmt.Sprint` d'une valeur YAML Go : ce que compose-go produit quand il
/// formate un scalaire en chaîne.
pub fn go_sprint(n: &Node) -> String {
    match &n.value {
        Value::Null => "<nil>".into(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => go_float(*f),
        Value::Str(s) => s.clone(),
        // `time.Time.String()`, l'horodatage étant décodé en time.Time.
        Value::Timestamp(t) => go_time_string(t),
        Value::Seq(_) | Value::Map(_) => format!("{:?}", n.to_json()),
    }
}

/// `strconv.FormatFloat(f, 'g', -1, 64)`, ce que `fmt.Sprint` fait d'un
/// float64 : plus court arrondi exact, exposant si < 1e-4 ou >= 1e21.
pub fn go_float(f: f64) -> String {
    if f.is_nan() {
        return "NaN".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "+Inf".into() } else { "-Inf".into() };
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    let exp = f.abs().log10().floor() as i32;
    if (-4..21).contains(&exp) {
        // Rust `{}` donne déjà la représentation la plus courte.
        let s = format!("{f}");
        s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
    } else {
        let s = format!("{f:e}");
        // Go écrit `1e+21`, Rust `1e21`.
        match s.split_once('e') {
            Some((m, e)) if !e.starts_with('-') => format!("{m}e+{e:0>2}"),
            Some((m, e)) => format!("{m}e-{:0>2}", &e[1..]),
            None => s,
        }
    }
}

/// `time.Time.String()` pour un horodatage RFC 3339 normalisé :
/// `2026-09-13 00:00:00 +0000 UTC`, ou `... +0200 +0200` avec un décalage
/// explicite (Go affiche l'abréviation de zone, qui vaut le décalage).
pub fn go_time_string(rfc3339: &str) -> String {
    let (date, rest) = rfc3339.split_once('T').unwrap_or((rfc3339, ""));
    if let Some(t) = rest.strip_suffix('Z') {
        return format!("{date} {t} +0000 UTC");
    }
    match rest.rfind(['+', '-']) {
        Some(idx) => {
            let (t, off) = rest.split_at(idx);
            let off = off.replace(':', "");
            format!("{date} {t} {off} {off}")
        }
        None => format!("{date} {rest} +0000 UTC"),
    }
}

fn append_without_duplicates(mut base: Vec<Node>, other: Vec<Node>) -> Vec<Node> {
    for v in other {
        if !base.iter().any(|e| same_value(e, &v)) {
            base.push(v);
        }
    }
    base
}

/// Égalité profonde des valeurs, positions et tags ignorés
/// (`reflect.DeepEqual` sur l'arbre Go).
pub fn same_value(a: &Node, b: &Node) -> bool {
    match (&a.value, &b.value) {
        (Value::Seq(x), Value::Seq(y)) => x.len() == y.len() && x.iter().zip(y).all(|(i, j)| same_value(i, j)),
        (Value::Map(x), Value::Map(y)) => x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same_value(v, w))),
        (x, y) => x == y,
    }
}

fn merge_as_mapping(base: Node, other: Node, defaults: &[(&str, Value)], p: &Path) -> Result<Node, Diagnostic> {
    let span = base.span;
    let right = into_mapping(base, defaults, p)?;
    let left = into_mapping(other, defaults, p)?;
    Ok(Node::new(span, Value::Map(merge_mappings(right, left, p)?)))
}

/// `convertIntoMapping` : une liste de noms devient un mapping vers les
/// valeurs par défaut (ou null sans défaut).
fn into_mapping(n: Node, defaults: &[(&str, Value)], p: &Path) -> Result<IndexMap<String, Node>, Diagnostic> {
    let span = n.span;
    match n.value {
        Value::Map(m) => Ok(m),
        Value::Seq(items) => {
            let mut out = IndexMap::new();
            for i in items {
                let Value::Str(k) = i.value else {
                    return Err(Diagnostic::error("merge-invalid-entry", format!("{p} : une chaîne est attendue")).at(p, i.span));
                };
                let v = if defaults.is_empty() {
                    Node::new(i.span, Value::Null)
                } else {
                    Node::new(i.span, Value::Map(defaults.iter().map(|(k, v)| (k.to_string(), Node::new(i.span, v.clone()))).collect()))
                };
                out.insert(k, v);
            }
            Ok(out)
        }
        Value::Null => Ok(IndexMap::new()),
        _ => Err(Diagnostic::error("merge-invalid-entry", format!("{p} : un mapping ou une liste est attendu")).at(p, span)),
    }
}

/// Les options de logging ne se fusionnent que si les deux fichiers
/// déclarent le même driver ; sinon l'override remplace tout.
fn merge_logging(base: Node, other: Node, p: &Path) -> Result<Node, Diagnostic> {
    let (Some(b), Some(o)) = (base.as_map(), other.as_map()) else {
        return Ok(other);
    };
    let driver = |m: &IndexMap<String, Node>| m.get("driver").and_then(Node::as_str).map(str::to_string);
    match (driver(b), driver(o)) {
        (Some(x), Some(y)) if x != y => {
            let mut o = o.clone();
            // Le driver change : on garde les options de l'override seul.
            if let Some(opts) = o.get("options").cloned() {
                o.insert("options".into(), opts);
            }
            Ok(Node::new(other.span, Value::Map(o)))
        }
        _ => {
            let Value::Map(bm) = base.value else { unreachable!() };
            let Value::Map(om) = other.value else { unreachable!() };
            Ok(Node::new(base.span, Value::Map(merge_mappings(bm, om, p)?)))
        }
    }
}

/// `mergeIPAMConfig` : les entrées sont identifiées par leur `subnet`.
fn merge_ipam_config(base: Node, other: Node) -> Result<Node, Diagnostic> {
    let mut out = seq_of(base.clone());
    for entry in seq_of(other) {
        let subnet = entry.as_map().and_then(|m| m.get("subnet")).and_then(Node::as_str).map(str::to_string);
        match subnet.and_then(|s| out.iter().position(|e| e.as_map().and_then(|m| m.get("subnet")).and_then(Node::as_str) == Some(s.as_str()))) {
            Some(i) => out[i] = entry,
            None => out.push(entry),
        }
    }
    Ok(Node::new(base.span, Value::Seq(out)))
}

/// `EnforceUnicity` : dans certaines listes, une entrée plus récente de
/// même clé remplace l'ancienne à sa position.
pub fn enforce_unicity(node: Node) -> Result<Node, Diagnostic> {
    unicity_at(node, &Path::root())
}

fn unicity_at(node: Node, p: &Path) -> Result<Node, Diagnostic> {
    match node.value {
        Value::Map(m) => {
            let mut out = IndexMap::with_capacity(m.len());
            for (k, v) in m {
                let u = unicity_at(v, &p.child(&k))?;
                out.insert(k, u);
            }
            Ok(Node { span: node.span, tag: node.tag, value: Value::Map(out) })
        }
        Value::Seq(items) => {
            let Some(indexer) = indexer_for(p) else {
                return Ok(Node { span: node.span, tag: node.tag, value: Value::Seq(items) });
            };
            let mut seq: Vec<Node> = Vec::new();
            let mut keys: IndexMap<String, usize> = IndexMap::new();
            for (i, entry) in items.into_iter().enumerate() {
                let key = indexer(&entry, &p.index(i))?;
                match keys.get(&key) {
                    Some(&j) => seq[j] = entry,
                    None => {
                        seq.push(entry);
                        keys.insert(key, seq.len() - 1);
                    }
                }
            }
            Ok(Node { span: node.span, tag: node.tag, value: Value::Seq(seq) })
        }
        _ => Ok(node),
    }
}

type Indexer = fn(&Node, &Path) -> Result<String, Diagnostic>;

fn indexer_for(p: &Path) -> Option<Indexer> {
    const KEY_VALUE: &[&str] = &[
        "networks.*.labels",
        "networks.*.ipam.options",
        "services.*.annotations",
        "services.*.build.args",
        "services.*.build.additional_contexts",
        "services.*.build.platform",
        "services.*.build.tags",
        "services.*.build.labels",
        "services.*.cap_add",
        "services.*.cap_drop",
        "services.*.deploy.labels",
        "services.*.dns",
        "services.*.dns_opt",
        "services.*.dns_search",
        "services.*.environment",
        "services.*.labels",
        "services.*.links",
        "services.*.networks.*.aliases",
        "services.*.networks.*.link_local_ips",
        "services.*.profiles",
        "services.*.sysctls",
        "services.*.tmpfs",
    ];
    if KEY_VALUE.iter().any(|pat| p.matches(pat)) {
        return Some(key_value_indexer);
    }
    if p.matches("services.*.volumes") {
        return Some(volume_indexer);
    }
    if p.matches("services.*.devices") {
        return Some(device_indexer);
    }
    if p.matches("services.*.env_file") {
        return Some(env_file_indexer);
    }
    if p.matches("services.*.expose") {
        return Some(expose_indexer);
    }
    if p.matches("services.*.ports") {
        return Some(port_indexer);
    }
    if p.matches("services.*.secrets") || p.matches("services.*.configs") {
        return Some(mount_indexer);
    }
    None
}

fn key_value_indexer(n: &Node, p: &Path) -> Result<String, Diagnostic> {
    match n.as_str() {
        Some(s) => Ok(s.split_once('=').map(|(k, _)| k.to_string()).unwrap_or_else(|| s.to_string())),
        None => Err(Diagnostic::error("unicity-invalid-entry", format!("{p} : une chaîne est attendue")).at(p, n.span)),
    }
}

fn volume_indexer(n: &Node, p: &Path) -> Result<String, Diagnostic> {
    match &n.value {
        Value::Map(m) => m
            .get("target")
            .and_then(Node::as_str)
            .map(str::to_string)
            .ok_or_else(|| Diagnostic::error("volume-missing-target", format!("le volume {p} n'a pas de cible (`target`)")).at(p, n.span)),
        Value::Str(s) => super::transform::parse_volume(s).map(|v| v.target).map_err(|e| Diagnostic::error("volume-invalid", format!("{p} : {e}")).at(p, n.span)),
        _ => Ok(String::new()),
    }
}

fn device_indexer(n: &Node, p: &Path) -> Result<String, Diagnostic> {
    match &n.value {
        Value::Map(m) => m
            .get("target")
            .and_then(Node::as_str)
            .map(str::to_string)
            .ok_or_else(|| Diagnostic::error("device-missing-target", format!("le device {p} n'a pas de cible (`target`)")).at(p, n.span)),
        Value::Str(s) => {
            let parts: Vec<&str> = s.split(':').collect();
            Ok(if parts.len() == 1 { parts[0].to_string() } else { parts[1].to_string() })
        }
        _ => Ok(String::new()),
    }
}

fn env_file_indexer(n: &Node, p: &Path) -> Result<String, Diagnostic> {
    match &n.value {
        Value::Str(s) => Ok(s.clone()),
        Value::Map(m) => m
            .get("path")
            .and_then(Node::as_str)
            .map(str::to_string)
            .ok_or_else(|| Diagnostic::error("env-file-missing-path", format!("{p} : `path` manquant")).at(p, n.span)),
        _ => Err(Diagnostic::error("unicity-invalid-entry", format!("{p} : type inattendu")).at(p, n.span)),
    }
}

fn expose_indexer(n: &Node, p: &Path) -> Result<String, Diagnostic> {
    match &n.value {
        Value::Str(s) => Ok(s.clone()),
        Value::Int(i) => Ok(i.to_string()),
        _ => Err(Diagnostic::error("unicity-invalid-entry", format!("{p} : type inattendu pour expose")).at(p, n.span)),
    }
}

fn port_indexer(n: &Node, p: &Path) -> Result<String, Diagnostic> {
    match &n.value {
        Value::Map(m) => {
            let target = m.get("target").map(go_sprint).unwrap_or_default();
            let published = m.get("published").map(go_sprint).unwrap_or_default();
            let ip = m.get("host_ip").map(go_sprint).unwrap_or_default();
            let protocol = m.get("protocol").map(go_sprint).unwrap_or_default();
            Ok(format!("{ip}:{published}:{target}/{protocol}"))
        }
        Value::Str(s) => Ok(s.clone()),
        Value::Int(i) => Ok(i.to_string()),
        _ => Err(Diagnostic::error("unicity-invalid-entry", format!("{p} : type inattendu pour ports")).at(p, n.span)),
    }
}

fn mount_indexer(n: &Node, p: &Path) -> Result<String, Diagnostic> {
    match &n.value {
        Value::Str(s) => Ok(s.clone()),
        Value::Map(m) => {
            if let Some(t) = m.get("target").and_then(Node::as_str) {
                return Ok(t.to_string());
            }
            m.get("source")
                .and_then(Node::as_str)
                .map(str::to_string)
                .ok_or_else(|| Diagnostic::error("mount-missing-source", format!("{p} : `source` manquant")).at(p, n.span))
        }
        _ => Err(Diagnostic::error("unicity-invalid-entry", format!("{p} : type inattendu")).at(p, n.span)),
    }
}
