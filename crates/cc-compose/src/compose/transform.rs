//! Canonicalisation : formes courtes vers formes longues, puis valeurs par
//! défaut. Port de `transform/*.go` de compose-go, et des parseurs
//! `types.ParsePortConfig` (via `nat.ParsePortSpecs`) et `format.ParseVolume`.

use indexmap::IndexMap;

use super::merge::go_sprint;
use super::node::{Node, Path, Span, Value};
use crate::diagnostic::Diagnostic;

fn map_node(span: Span, entries: Vec<(&str, Node)>) -> Node {
    Node::new(span, Value::Map(entries.into_iter().map(|(k, v)| (k.to_string(), v)).collect()))
}

fn str_node(span: Span, s: impl Into<String>) -> Node {
    Node::new(span, Value::Str(s.into()))
}

/// `transform.Canonical`.
pub fn canonical(node: Node) -> Result<Node, Diagnostic> {
    canonical_at(node, &Path::root())
}

fn canonical_at(node: Node, p: &Path) -> Result<Node, Diagnostic> {
    // `transformers` de compose-go. Le premier motif qui correspond gagne et
    // la descente s'arrête, sauf pour les transformations qui descendent
    // elles-mêmes (services.*, build, externals).
    if p.matches("services.*") {
        return match node.value {
            Value::Map(_) => descend(node, p),
            _ => Ok(node),
        };
    }
    if p.matches("services.*.build") {
        return match node.value {
            Value::Map(_) => descend(node, p),
            Value::Str(s) => Ok(map_node(node.span, vec![("context", str_node(node.span, s))])),
            _ => Err(Diagnostic::error("build-invalid", format!("{p} : type invalide pour build")).at(p, node.span)),
        };
    }
    if p.matches("services.*.build.secrets.*") || p.matches("services.*.secrets.*") || p.matches("services.*.configs.*") {
        return file_mount(node, p);
    }
    if p.matches("services.*.build.provenance") || p.matches("services.*.build.sbom") {
        return Ok(match node.value {
            Value::Str(_) => node,
            _ => str_node(node.span, go_sprint(&node)),
        });
    }
    if p.matches("services.*.build.additional_contexts") {
        return key_value(node, p);
    }
    if p.matches("services.*.depends_on") {
        return depends_on(node, p);
    }
    if p.matches("services.*.env_file") {
        return env_file(node, p);
    }
    if p.matches("services.*.label_file") || p.matches("services.*.dns") || p.matches("services.*.develop.watch.*.ignore") || p.matches("services.*.develop.watch.*.include") {
        return Ok(string_or_list(node));
    }
    if p.matches("services.*.extends") {
        return Ok(match node.value {
            Value::Str(s) => map_node(node.span, vec![("service", str_node(node.span, s))]),
            _ => node,
        });
    }
    if p.matches("services.*.gpus") {
        return Ok(match node.value {
            Value::Str(ref s) if s == "all" => Node::new(node.span, Value::Seq(vec![map_node(node.span, vec![("count", str_node(node.span, "all"))])])),
            _ => node,
        });
    }
    if p.matches("services.*.networks") || p.matches("services.*.models") {
        return Ok(match node.value {
            Value::Seq(items) => {
                let mut m = IndexMap::new();
                for i in items {
                    m.insert(go_sprint(&i), Node::new(i.span, Value::Null));
                }
                Node::new(node.span, Value::Map(m))
            }
            _ => node,
        });
    }
    if p.matches("services.*.volumes.*") {
        return volume_mount(node, p);
    }
    if p.matches("services.*.devices.*") {
        return device_mapping(node, p);
    }
    if p.matches("services.*.ports") {
        return ports(node, p);
    }
    if p.matches("services.*.build.ssh") {
        return ssh(node, p);
    }
    if p.matches("services.*.ulimits.*") || p.matches("services.*.build.ulimits.*") {
        return Ok(node);
    }
    if p.matches("volumes.*") || p.matches("networks.*") || p.matches("secrets.*") || p.matches("configs.*") {
        return maybe_external(node, p);
    }
    if p.matches("include.*") {
        return Ok(match node.value {
            Value::Str(s) => map_node(node.span, vec![("path", str_node(node.span, s))]),
            _ => node,
        });
    }
    descend(node, p)
}

fn descend(node: Node, p: &Path) -> Result<Node, Diagnostic> {
    Ok(match node.value {
        Value::Map(m) => {
            let mut out = IndexMap::with_capacity(m.len());
            for (k, v) in m {
                let t = canonical_at(v, &p.child(&k))?;
                out.insert(k, t);
            }
            Node { span: node.span, tag: node.tag, value: Value::Map(out) }
        }
        Value::Seq(s) => {
            let mut out = Vec::with_capacity(s.len());
            for (i, v) in s.into_iter().enumerate() {
                out.push(canonical_at(v, &p.index(i))?);
            }
            Node { span: node.span, tag: node.tag, value: Value::Seq(out) }
        }
        _ => node,
    })
}

fn string_or_list(node: Node) -> Node {
    match node.value {
        Value::Str(s) => Node::new(node.span, Value::Seq(vec![str_node(node.span, s)])),
        _ => node,
    }
}

fn key_value(node: Node, p: &Path) -> Result<Node, Diagnostic> {
    match node.value {
        Value::Map(_) => Ok(node),
        Value::Seq(items) => {
            let mut m = IndexMap::new();
            for i in items {
                let s = go_sprint(&i);
                let Some((k, v)) = s.split_once('=') else {
                    return Err(Diagnostic::error("key-value-invalid", format!("{p} : valeur invalide `{s}`, `clé=valeur` attendu")).at(p, i.span));
                };
                m.insert(k.to_string(), str_node(i.span, v));
            }
            Ok(Node::new(node.span, Value::Map(m)))
        }
        _ => Err(Diagnostic::error("key-value-invalid", format!("{p} : type invalide")).at(p, node.span)),
    }
}

fn depends_on(node: Node, p: &Path) -> Result<Node, Diagnostic> {
    let span = node.span;
    match node.value {
        Value::Map(m) => {
            let mut out = IndexMap::new();
            for (k, v) in m {
                let Value::Map(mut d) = v.value else {
                    return Err(Diagnostic::error("depends-on-invalid", format!("{p}.{k} : valeur non supportée")).at(p, v.span));
                };
                d.entry("condition".into()).or_insert_with(|| str_node(v.span, "service_started"));
                d.entry("required".into()).or_insert_with(|| Node::new(v.span, Value::Bool(true)));
                out.insert(k, Node::new(v.span, Value::Map(d)));
            }
            Ok(Node::new(span, Value::Map(out)))
        }
        Value::Seq(items) => {
            let mut out = IndexMap::new();
            for i in items {
                out.insert(
                    go_sprint(&i),
                    map_node(i.span, vec![("condition", str_node(i.span, "service_started")), ("required", Node::new(i.span, Value::Bool(true)))]),
                );
            }
            Ok(Node::new(span, Value::Map(out)))
        }
        _ => Err(Diagnostic::error("depends-on-invalid", format!("{p} : type invalide pour depends_on")).at(p, span)),
    }
}

fn env_file(node: Node, p: &Path) -> Result<Node, Diagnostic> {
    let one = |n: Node| -> Node {
        match n.value {
            Value::Str(s) => map_node(n.span, vec![("path", str_node(n.span, s)), ("required", Node::new(n.span, Value::Bool(true)))]),
            Value::Map(mut m) => {
                m.entry("required".into()).or_insert_with(|| Node::new(n.span, Value::Bool(true)));
                Node::new(n.span, Value::Map(m))
            }
            _ => Node::new(n.span, Value::Null),
        }
    };
    match node.value {
        Value::Str(_) => Ok(Node::new(node.span, Value::Seq(vec![one(node)]))),
        Value::Seq(items) => Ok(Node::new(node.span, Value::Seq(items.into_iter().map(one).collect()))),
        _ => Err(Diagnostic::error("env-file-invalid", format!("{p} : type invalide pour env_file")).at(p, node.span)),
    }
}

fn file_mount(node: Node, p: &Path) -> Result<Node, Diagnostic> {
    match node.value {
        Value::Map(_) => Ok(node),
        Value::Str(s) => Ok(map_node(node.span, vec![("source", str_node(node.span, s))])),
        _ => Err(Diagnostic::error("mount-invalid", format!("{p} : type non supporté")).at(p, node.span)),
    }
}

fn ssh(node: Node, p: &Path) -> Result<Node, Diagnostic> {
    match node.value {
        Value::Map(_) => Ok(node),
        Value::Str(s) => Ok(map_node(node.span, vec![(&s.clone(), str_node(node.span, ""))]).with_ssh_split(s)),
        Value::Seq(items) => {
            let mut m = IndexMap::new();
            for i in items {
                let s = go_sprint(&i);
                let (k, v) = s.split_once('=').unwrap_or((&s, ""));
                m.insert(k.to_string(), str_node(i.span, v));
            }
            Ok(Node::new(node.span, Value::Map(m)))
        }
        _ => Err(Diagnostic::error("ssh-invalid", format!("{p} : type invalide pour build.ssh")).at(p, node.span)),
    }
}

impl Node {
    fn with_ssh_split(self, s: String) -> Node {
        let (k, v) = s.split_once('=').unwrap_or((&s, ""));
        map_node(self.span, vec![(k, str_node(self.span, v))])
    }
}

fn device_mapping(node: Node, p: &Path) -> Result<Node, Diagnostic> {
    match node.value {
        Value::Map(_) => Ok(node),
        Value::Str(s) => {
            let parts: Vec<&str> = s.split(':').collect();
            let span = node.span;
            let (src, dst, permissions) = match parts.len() {
                1 => (parts[0], parts[0], "rwm"),
                2 => (parts[0], parts[1], "rwm"),
                3 => (parts[0], parts[1], parts[2]),
                _ => return Err(Diagnostic::error("device-invalid", format!("{p} : device invalide `{s}`, utilisez la syntaxe longue")).at(p, span)),
            };
            let dst = if dst.is_empty() { src } else { dst };
            Ok(map_node(span, vec![("source", str_node(span, src)), ("target", str_node(span, dst)), ("permissions", str_node(span, permissions))]))
        }
        _ => Err(Diagnostic::error("device-invalid", format!("{p} : type invalide pour device")).at(p, node.span)),
    }
}

/// `transformMaybeExternal` : descente, puis `external: {name: x}` déprécié
/// devient `external: true` + `name: x`.
fn maybe_external(node: Node, p: &Path) -> Result<Node, Diagnostic> {
    if node.is_null() {
        return Ok(node);
    }
    let node = descend(node, p)?;
    let Value::Map(mut m) = node.value else {
        return Ok(Node { span: node.span, tag: node.tag, value: node.value });
    };
    if let Some(ext) = m.get("external").cloned() {
        if let Value::Map(em) = ext.value {
            m.insert("external".into(), Node::new(ext.span, Value::Bool(true)));
            if let Some(extname) = em.get("name") {
                match m.get("name") {
                    Some(name) if !super::merge::same_value(name, extname) => {
                        return Err(Diagnostic::error("external-name-conflict", format!("{p} : `name` et `external.name` sont en conflit ; n'utilisez que `name`")).at(p, ext.span));
                    }
                    Some(_) => {}
                    None => {
                        m.insert("name".into(), extname.clone());
                    }
                }
            }
        }
    }
    Ok(Node { span: node.span, tag: node.tag, value: Value::Map(m) })
}

// --- ports -----------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortConfig {
    pub host_ip: String,
    pub protocol: String,
    pub target: u32,
    pub published: String,
}

fn ports(node: Node, p: &Path) -> Result<Node, Diagnostic> {
    let Value::Seq(entries) = node.value else {
        return Err(Diagnostic::error("ports-invalid", format!("{p} : type invalide pour ports")).at(p, node.span));
    };
    let mut out = Vec::new();
    for e in entries {
        match &e.value {
            Value::Int(_) | Value::Str(_) => {
                let spec = go_sprint(&e);
                let parsed = parse_port_config(&spec).map_err(|m| Diagnostic::error("ports-invalid", format!("{p} : {m}")).at(p, e.span))?;
                for pc in parsed {
                    let mut entries = Vec::new();
                    entries.push(("mode", str_node(e.span, "ingress")));
                    if !pc.host_ip.is_empty() {
                        entries.push(("host_ip", str_node(e.span, pc.host_ip)));
                    }
                    entries.push(("target", Node::new(e.span, Value::Int(pc.target as i64))));
                    if !pc.published.is_empty() {
                        entries.push(("published", str_node(e.span, pc.published)));
                    }
                    entries.push(("protocol", str_node(e.span, pc.protocol)));
                    out.push(map_node(e.span, entries));
                }
            }
            Value::Map(_) => out.push(e),
            _ => return Err(Diagnostic::error("ports-invalid", format!("{p} : type invalide pour un port")).at(p, e.span)),
        }
    }
    Ok(Node::new(node.span, Value::Seq(out)))
}

/// `types.ParsePortConfig` : `nat.ParsePortSpecs` sur une entrée, puis les
/// ports triés par leur clé `port/proto` (tri de chaînes).
pub fn parse_port_config(value: &str) -> Result<Vec<PortConfig>, String> {
    let mut configs = parse_port_spec(value)?;
    configs.sort_by(|a, b| format!("{}/{}", a.target, a.protocol).cmp(&format!("{}/{}", b.target, b.protocol)));
    Ok(configs)
}

/// `nat.ParsePortSpec` (go-connections) : `[ip:][hostPort:]containerPort[/proto]`,
/// plages `a-b` des deux côtés.
fn parse_port_spec(raw: &str) -> Result<Vec<PortConfig>, String> {
    let (spec, proto) = match raw.rsplit_once('/') {
        Some((s, p)) if !p.is_empty() && !p.contains(':') => (s, p.to_lowercase()),
        _ => (raw, "tcp".to_string()),
    };
    if !["tcp", "udp", "sctp"].contains(&proto.as_str()) {
        return Err(format!("protocole invalide `{proto}` dans `{raw}`"));
    }
    // Séparation ip / hostPort / containerPort, l'IPv6 entre crochets.
    let (ip, host_port, container_port) = split_parts(spec)?;
    if container_port.is_empty() {
        return Err(format!("aucun port dans `{raw}`"));
    }
    if !ip.is_empty() && ip.parse::<std::net::IpAddr>().is_err() {
        return Err(format!("adresse IP invalide `{ip}` dans `{raw}`"));
    }
    let (c_start, c_end) = parse_range(container_port)?;
    let (h_start, h_end) = if host_port.is_empty() { (0, 0) } else { parse_range(host_port)? };
    if !host_port.is_empty() && (c_end - c_start) != (h_end - h_start) && (h_end - h_start) != 0 {
        return Err(format!("les plages de ports `{host_port}` et `{container_port}` n'ont pas la même taille"));
    }
    let mut out = Vec::new();
    for i in 0..=(c_end - c_start) {
        let target = c_start + i;
        let published = if host_port.is_empty() {
            String::new()
        } else if h_end - h_start == 0 && c_end - c_start > 0 {
            // Un seul port hôte pour une plage conteneur : go-connections
            // garde la valeur hôte telle quelle.
            host_port.to_string()
        } else {
            (h_start + i).to_string()
        };
        out.push(PortConfig { host_ip: ip.to_string(), protocol: proto.clone(), target, published });
    }
    Ok(out)
}

fn split_parts(spec: &str) -> Result<(&str, &str, &str), String> {
    // `nat.SplitParts` : au plus deux `:` hors crochets IPv6.
    let (ip, rest) = if let Some(r) = spec.strip_prefix('[') {
        let end = r.find(']').ok_or_else(|| format!("crochet non fermé dans `{spec}`"))?;
        let rest = r[end + 1..].strip_prefix(':').ok_or_else(|| format!("`:` attendu après l'adresse dans `{spec}`"))?;
        (&r[..end], rest)
    } else {
        let parts: Vec<&str> = spec.split(':').collect();
        match parts.len() {
            1 => return Ok(("", "", parts[0])),
            2 => return Ok(("", parts[0], parts[1])),
            3 => (parts[0], &spec[parts[0].len() + 1..]),
            _ => return Err(format!("format de port invalide `{spec}`")),
        }
    };
    let parts: Vec<&str> = rest.split(':').collect();
    match parts.len() {
        1 => Ok((ip, "", parts[0])),
        2 => Ok((ip, parts[0], parts[1])),
        _ => Err(format!("format de port invalide `{spec}`")),
    }
}

fn parse_range(s: &str) -> Result<(u32, u32), String> {
    let port = |x: &str| x.parse::<u32>().ok().filter(|p| *p <= 65535).ok_or_else(|| format!("port invalide `{x}`"));
    match s.split_once('-') {
        Some((a, b)) => {
            let (a, b) = (port(a)?, port(b)?);
            if a > b {
                return Err(format!("plage de ports invalide `{s}`"));
            }
            Ok((a, b))
        }
        None => {
            let p = port(s)?;
            Ok((p, p))
        }
    }
}

// --- volumes ---------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VolumeConfig {
    pub kind: String,
    pub source: String,
    pub target: String,
    pub read_only: bool,
    pub nocopy: bool,
    pub propagation: Option<String>,
    pub selinux: Option<String>,
    pub create_host_path: bool,
}

fn volume_mount(node: Node, p: &Path) -> Result<Node, Diagnostic> {
    match node.value {
        Value::Map(_) => Ok(node),
        Value::Str(s) => {
            let mut v = parse_volume(&s).map_err(|m| Diagnostic::error("volume-invalid", format!("{p} : {m}")).at(p, node.span))?;
            v.target = clean_path(&v.target);
            Ok(encode_volume(&v, node.span))
        }
        _ => Err(Diagnostic::error("volume-invalid", format!("{p} : type invalide pour un montage")).at(p, node.span)),
    }
}

/// Encodage mapstructure avec `omitempty` : les champs vides disparaissent,
/// les sous-structures présentes mais vides restent (`volume: {}`).
fn encode_volume(v: &VolumeConfig, span: Span) -> Node {
    let mut entries: Vec<(&str, Node)> = vec![("type", str_node(span, &v.kind))];
    if !v.source.is_empty() {
        entries.push(("source", str_node(span, &v.source)));
    }
    if !v.target.is_empty() {
        entries.push(("target", str_node(span, &v.target)));
    }
    if v.read_only {
        entries.push(("read_only", Node::new(span, Value::Bool(true))));
    }
    match v.kind.as_str() {
        "bind" => {
            let mut bind = Vec::new();
            if let Some(pr) = &v.propagation {
                bind.push(("propagation", str_node(span, pr)));
            }
            if let Some(se) = &v.selinux {
                bind.push(("selinux", str_node(span, se)));
            }
            if v.create_host_path {
                bind.push(("create_host_path", Node::new(span, Value::Bool(true))));
            }
            entries.push(("bind", map_node(span, bind)));
        }
        _ => {
            let mut vol = Vec::new();
            if v.nocopy {
                vol.push(("nocopy", Node::new(span, Value::Bool(true))));
            }
            entries.push(("volume", map_node(span, vol)));
        }
    }
    map_node(span, entries)
}

/// `format.ParseVolume` : `[source:]target[:options]`, avec la lettre de
/// lecteur Windows et les `${...}` tolérés dans les sections.
pub fn parse_volume(spec: &str) -> Result<VolumeConfig, String> {
    let mut v = VolumeConfig::default();
    match spec.chars().count() {
        0 => return Err("spécification de volume vide".into()),
        1 | 2 => {
            v.target = spec.to_string();
            v.kind = "volume".into();
            return Ok(v);
        }
        _ => {}
    }
    let chars: Vec<char> = spec.chars().collect();
    let mut buffer: Vec<char> = Vec::new();
    let mut depth = 0usize;
    let mut sections: Vec<String> = Vec::new();
    for (i, &c) in chars.iter().enumerate() {
        if c == '$' && chars.get(i + 1) == Some(&'{') {
            depth += 1;
            buffer.push(c);
            continue;
        }
        if c == '}' && depth > 0 {
            depth -= 1;
            buffer.push(c);
            continue;
        }
        let windows_drive = c == ':' && buffer.len() == 1 && buffer[0].is_alphabetic();
        if c == ':' && depth == 0 && !windows_drive {
            if buffer.is_empty() {
                return Err(format!("spécification invalide : {spec} : section vide entre deux `:`"));
            }
            sections.push(buffer.iter().collect());
            buffer.clear();
        } else {
            buffer.push(c);
        }
    }
    if buffer.is_empty() {
        return Err(format!("spécification invalide : {spec} : section vide entre deux `:`"));
    }
    sections.push(buffer.iter().collect());
    match sections.len() {
        1 => v.target = sections.remove(0),
        2 => {
            v.source = sections.remove(0);
            v.target = sections.remove(0);
        }
        3 => {
            v.source = sections.remove(0);
            v.target = sections.remove(0);
            for option in sections.remove(0).split(',') {
                match option {
                    "ro" => v.read_only = true,
                    "rw" => v.read_only = false,
                    "nocopy" => v.nocopy = true,
                    "rprivate" | "private" | "rshared" | "shared" | "rslave" | "slave" => v.propagation = Some(option.into()),
                    "z" | "Z" => v.selinux = Some(option.into()),
                    // Les options inconnues (cached, delegated...) sont ignorées.
                    _ => {}
                }
            }
        }
        _ => return Err(format!("spécification invalide : {spec} : trop de `:`")),
    }
    if is_file_path(&v.source) {
        v.kind = "bind".into();
        v.create_host_path = true;
    } else {
        v.kind = "volume".into();
    }
    Ok(v)
}

fn is_file_path(source: &str) -> bool {
    let b = source.as_bytes();
    if b.is_empty() {
        return false;
    }
    match b[0] {
        b'.' | b'/' | b'~' => true,
        // Lecteur Windows `C:\` ou `C:/`.
        c if c.is_ascii_alphabetic() && b.len() >= 3 && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/') => true,
        _ => false,
    }
}

/// `path.Clean` de Go (chemins POSIX).
pub fn clean_path(p: &str) -> String {
    if p.is_empty() {
        return String::new();
    }
    let rooted = p.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if out.last().is_some_and(|s| *s != "..") {
                    out.pop();
                } else if !rooted {
                    out.push("..");
                }
            }
            s => out.push(s),
        }
    }
    let joined = out.join("/");
    match (rooted, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".into(),
        (false, false) => joined,
    }
}

// --- valeurs par défaut ----------------------------------------------------

/// `transform.SetDefaultValues`.
pub fn set_defaults(node: Node) -> Result<Node, Diagnostic> {
    defaults_at(node, &Path::root())
}

fn defaults_at(node: Node, p: &Path) -> Result<Node, Diagnostic> {
    if p.matches("services.*.build") {
        return Ok(match node.value {
            Value::Map(mut m) => {
                m.entry("context".into()).or_insert_with(|| str_node(node.span, "."));
                Node::new(node.span, Value::Map(m))
            }
            _ => node,
        });
    }
    if p.matches("services.*.secrets.*") {
        return match node.value {
            Value::Map(mut m) => {
                if !m.contains_key("target") {
                    let source = m.get("source").map(go_sprint).unwrap_or_default();
                    m.insert("target".into(), str_node(node.span, format!("/run/secrets/{source}")));
                }
                Ok(Node::new(node.span, Value::Map(m)))
            }
            _ => Err(Diagnostic::error("secret-invalid", format!("{p} : type non supporté")).at(p, node.span)),
        };
    }
    if p.matches("services.*.ports.*") {
        return Ok(match node.value {
            Value::Map(mut m) => {
                m.entry("protocol".into()).or_insert_with(|| str_node(node.span, "tcp"));
                m.entry("mode".into()).or_insert_with(|| str_node(node.span, "ingress"));
                Node::new(node.span, Value::Map(m))
            }
            _ => node,
        });
    }
    if p.matches("services.*.deploy.resources.reservations.devices.*") || p.matches("services.*.gpus.*") {
        return Ok(match node.value {
            Value::Map(mut m) => {
                if !m.contains_key("count") && !m.contains_key("device_ids") {
                    m.insert("count".into(), str_node(node.span, "all"));
                }
                Node::new(node.span, Value::Map(m))
            }
            _ => node,
        });
    }
    if p.matches("services.*.volumes.*.bind") {
        return match node.value {
            Value::Map(mut m) => {
                m.entry("create_host_path".into()).or_insert_with(|| Node::new(node.span, Value::Bool(true)));
                Ok(Node::new(node.span, Value::Map(m)))
            }
            _ => Err(Diagnostic::error("volume-bind-invalid", format!("{p} : type invalide pour bind")).at(p, node.span)),
        };
    }
    Ok(match node.value {
        Value::Map(m) => {
            let mut out = IndexMap::with_capacity(m.len());
            for (k, v) in m {
                let t = defaults_at(v, &p.child(&k))?;
                out.insert(k, t);
            }
            Node { span: node.span, tag: node.tag, value: Value::Map(out) }
        }
        Value::Seq(s) => {
            let mut out = Vec::with_capacity(s.len());
            for (i, v) in s.into_iter().enumerate() {
                out.push(defaults_at(v, &p.index(i))?);
            }
            Node { span: node.span, tag: node.tag, value: Value::Seq(out) }
        }
        _ => node,
    })
}

/// `loader.OmitEmpty` : ne concerne que `services.*.dns`, dont les entrées
/// nulles ou vides sont retirées.
pub fn omit_empty(node: Node) -> Node {
    omit_at(node, &Path::root())
}

fn omit_at(node: Node, p: &Path) -> Node {
    let is_empty = |n: &Node| n.is_null() || n.as_str() == Some("");
    match node.value {
        Value::Map(m) => {
            let mut out = IndexMap::with_capacity(m.len());
            for (k, v) in m {
                if is_empty(&v) && p.matches("services.*.dns") {
                    continue;
                }
                let cp = p.child(&k);
                out.insert(k, omit_at(v, &cp));
            }
            Node { span: node.span, tag: node.tag, value: Value::Map(out) }
        }
        Value::Seq(s) => {
            let mut out = Vec::with_capacity(s.len());
            for (i, v) in s.into_iter().enumerate() {
                if is_empty(&v) && p.matches("services.*.dns") {
                    continue;
                }
                out.push(omit_at(v, &p.index(i)));
            }
            Node { span: node.span, tag: node.tag, value: Value::Seq(out) }
        }
        _ => node,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_specs() {
        let one = |s: &str| parse_port_config(s).unwrap();
        assert_eq!(one("8080:80"), vec![PortConfig { host_ip: "".into(), protocol: "tcp".into(), target: 80, published: "8080".into() }]);
        assert_eq!(one("9000")[0].published, "");
        assert_eq!(one("127.0.0.1:5432:5432/udp")[0], PortConfig { host_ip: "127.0.0.1".into(), protocol: "udp".into(), target: 5432, published: "5432".into() });
        let range = one("8000-8002:80-82");
        assert_eq!(range.iter().map(|p| (p.target, p.published.as_str())).collect::<Vec<_>>(), vec![(80, "8000"), (81, "8001"), (82, "8002")]);
        assert_eq!(one("[::1]:80:80")[0].host_ip, "::1");
        assert!(parse_port_config("70000").is_err());
        assert!(parse_port_config("a:b").is_err());
    }

    #[test]
    fn volume_specs() {
        let v = parse_volume("db_data:/var/lib/mysql").unwrap();
        assert_eq!((v.kind.as_str(), v.source.as_str(), v.target.as_str()), ("volume", "db_data", "/var/lib/mysql"));
        let v = parse_volume("./conf:/etc/nginx/conf.d:ro,z").unwrap();
        assert_eq!(v.kind, "bind");
        assert!(v.read_only && v.create_host_path);
        assert_eq!(v.selinux.as_deref(), Some("z"));
        assert_eq!(parse_volume("/data").unwrap().target, "/data");
        assert_eq!(parse_volume("${HOME}/x:/x").unwrap().source, "${HOME}/x");
        assert!(parse_volume("ab:cd:ef:gh").is_err());
        assert!(parse_volume("ab::c").is_err());
        assert_eq!(clean_path("/var//lib/../log/"), "/var/log");
        assert_eq!(clean_path("./x"), "x");
    }
}
