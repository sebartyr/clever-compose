//! Effets du modèle typé de compose-go sur l'arbre.
//!
//! `docker compose config` sérialise `types.Project`, dont plusieurs champs
//! ont un décodeur (`DecodeMapstructure`) qui change la forme : liste
//! `k=v` vers mapping, commande texte vers liste, nombres vers chaînes,
//! tailles en octets, durées normalisées. Cette passe applique ces
//! conversions sur l'arbre canonique, chemin par chemin, pour que le plan
//! lise la même chose que Compose et que le test différentiel compare des
//! formes identiques.

use indexmap::IndexMap;

use super::merge::go_sprint;
use super::node::{Node, Path, Span, Value};
use crate::diagnostic::Diagnostic;

pub fn apply(node: Node) -> Result<Node, Diagnostic> {
    at(node, &Path::root())
}

fn at(node: Node, p: &Path) -> Result<Node, Diagnostic> {
    // MappingWithEquals : valeur nulle possible.
    const MAPPING_WITH_EQUALS: &[&str] = &["services.*.environment", "services.*.build.args"];
    // Mapping / Labels / Options : valeurs toujours chaînes (`""` pour null).
    const STRING_MAPPING: &[&str] = &[
        "services.*.labels",
        "services.*.annotations",
        "services.*.sysctls",
        "services.*.build.labels",
        "services.*.deploy.labels",
        "services.*.logging.options",
        "networks.*.labels",
        "networks.*.driver_opts",
        "networks.*.ipam.options",
        "networks.*.ipam.config.*.aux_addresses",
        "volumes.*.labels",
        "volumes.*.driver_opts",
        "secrets.*.labels",
        "configs.*.labels",
        "services.*.volumes.*.volume.labels",
    ];
    const SHELL_COMMAND: &[&str] = &["services.*.command", "services.*.entrypoint"];
    const STRING_OR_NUMBER_LIST: &[&str] = &["services.*.expose"];
    const STRING_LIST: &[&str] = &["services.*.dns", "services.*.dns_search", "services.*.dns_opt", "services.*.tmpfs", "services.*.cap_add", "services.*.cap_drop", "services.*.security_opt", "services.*.group_add", "services.*.profiles"];
    const UNIT_BYTES: &[&str] = &["services.*.mem_limit", "services.*.mem_reservation", "services.*.memswap_limit", "services.*.shm_size", "services.*.deploy.resources.limits.memory", "services.*.deploy.resources.reservations.memory", "services.*.volumes.*.tmpfs.size"];
    const DURATION: &[&str] = &[
        "services.*.stop_grace_period",
        "services.*.healthcheck.interval",
        "services.*.healthcheck.timeout",
        "services.*.healthcheck.start_period",
        "services.*.healthcheck.start_interval",
        "services.*.deploy.update_config.delay",
        "services.*.deploy.update_config.monitor",
        "services.*.deploy.rollback_config.delay",
        "services.*.deploy.rollback_config.monitor",
        "services.*.deploy.restart_policy.delay",
        "services.*.deploy.restart_policy.window",
    ];

    // `NanoCPUs` (compose-go `types/cpus.go`) : chaîne ou nombre, rendu en
    // flottant.
    const NANO_CPUS: &[&str] = &["services.*.cpus", "services.*.deploy.resources.limits.cpus", "services.*.deploy.resources.reservations.cpus"];
    if NANO_CPUS.iter().any(|pat| p.matches(pat)) {
        return nano_cpus(node, p);
    }
    if MAPPING_WITH_EQUALS.iter().any(|pat| p.matches(pat)) {
        return mapping_with_equals(node, p);
    }
    if STRING_MAPPING.iter().any(|pat| p.matches(pat)) {
        return string_mapping(node, p);
    }
    if SHELL_COMMAND.iter().any(|pat| p.matches(pat)) {
        return shell_command(node, p);
    }
    if p.matches("services.*.extra_hosts") {
        return hosts_list(node, p);
    }
    if p.matches("services.*.secrets.[].mode") || p.matches("services.*.configs.[].mode") || p.matches("services.*.build.secrets.[].mode") {
        // `FileMode` : entier (YAML `0400` est déjà octal) ou chaîne octale,
        // rendu `0%o`.
        let value = match &node.value {
            Value::Int(i) => Some(*i),
            Value::Str(st) => i64::from_str_radix(st, 8).ok(),
            _ => None,
        };
        return match value {
            Some(v) => Ok(str_node(node.span, format!("0{v:o}"))),
            None => Err(Diagnostic::error("mode-invalid", format!("{p} : mode de fichier invalide")).at(p, node.span)),
        };
    }
    if p.matches("services.*.healthcheck.test") {
        return Ok(match node.value {
            Value::Str(s) => Node::new(node.span, Value::Seq(vec![str_node(node.span, "CMD-SHELL"), str_node(node.span, s)])),
            _ => node,
        });
    }
    if STRING_OR_NUMBER_LIST.iter().any(|pat| p.matches(pat)) {
        return Ok(match node.value {
            Value::Seq(items) => Node::new(node.span, Value::Seq(items.iter().map(|i| str_node(i.span, go_sprint(i))).collect())),
            Value::Str(_) => Node::new(node.span, Value::Seq(vec![node])),
            _ => node,
        });
    }
    if STRING_LIST.iter().any(|pat| p.matches(pat)) {
        return Ok(match node.value {
            Value::Str(_) => Node::new(node.span, Value::Seq(vec![node])),
            _ => node,
        });
    }
    if UNIT_BYTES.iter().any(|pat| p.matches(pat)) {
        return unit_bytes(node, p);
    }
    if DURATION.iter().any(|pat| p.matches(pat)) {
        return duration(node, p);
    }
    Ok(match node.value {
        Value::Map(m) => {
            let mut out = IndexMap::with_capacity(m.len());
            for (k, v) in m {
                let t = at(v, &p.child(&k))?;
                out.insert(k, t);
            }
            Node { span: node.span, tag: node.tag, value: Value::Map(out) }
        }
        Value::Seq(s) => {
            let mut out = Vec::with_capacity(s.len());
            for (i, v) in s.into_iter().enumerate() {
                out.push(at(v, &p.index(i))?);
            }
            Node { span: node.span, tag: node.tag, value: Value::Seq(out) }
        }
        _ => node,
    })
}

fn str_node(span: Span, s: impl Into<String>) -> Node {
    Node::new(span, Value::Str(s.into()))
}

fn mapping_with_equals(node: Node, p: &Path) -> Result<Node, Diagnostic> {
    let span = node.span;
    match node.value {
        Value::Map(m) => {
            let mut out = IndexMap::with_capacity(m.len());
            for (k, v) in m {
                let v = if v.is_null() { v } else { str_node(v.span, go_sprint(&v)) };
                out.insert(k, v);
            }
            Ok(Node::new(span, Value::Map(out)))
        }
        Value::Seq(items) => {
            let mut out = IndexMap::with_capacity(items.len());
            for i in items {
                let s = go_sprint(&i);
                let (k, v) = match s.split_once('=') {
                    Some((k, v)) => (k.to_string(), Some(v.to_string())),
                    None => (s.clone(), None),
                };
                if k.chars().last().is_some_and(char::is_whitespace) {
                    return Err(Diagnostic::error("environment-trailing-space", format!("la variable d'environnement `{k}` est déclarée avec une espace finale")).at(p, i.span));
                }
                out.insert(k, match v {
                    Some(v) => str_node(i.span, v),
                    None => Node::new(i.span, Value::Null),
                });
            }
            Ok(Node::new(span, Value::Map(out)))
        }
        Value::Null => Ok(node),
        _ => Err(Diagnostic::error("mapping-invalid", format!("{p} : type inattendu pour un mapping")).at(p, span)),
    }
}

fn string_mapping(node: Node, p: &Path) -> Result<Node, Diagnostic> {
    let span = node.span;
    match node.value {
        Value::Map(m) => {
            let mut out = IndexMap::with_capacity(m.len());
            for (k, v) in m {
                let s = if v.is_null() { String::new() } else { go_sprint(&v) };
                out.insert(k, str_node(v.span, s));
            }
            Ok(Node::new(span, Value::Map(out)))
        }
        Value::Seq(items) => {
            let mut out = IndexMap::with_capacity(items.len());
            for i in items {
                let s = go_sprint(&i);
                let (k, v) = s.split_once('=').unwrap_or((&s, ""));
                out.insert(k.to_string(), str_node(i.span, v));
            }
            Ok(Node::new(span, Value::Map(out)))
        }
        Value::Null => Ok(node),
        _ => Err(Diagnostic::error("mapping-invalid", format!("{p} : type inattendu pour un mapping")).at(p, span)),
    }
}

/// `HostsList` : entrées `host=ip` ou `host:ip` (ou mapping host → ip ou
/// liste d'ips), IPs sans crochets, rendues `host=ip` triées.
fn hosts_list(node: Node, p: &Path) -> Result<Node, Diagnostic> {
    let span = node.span;
    let mut hosts: IndexMap<String, Vec<String>> = IndexMap::new();
    let mut add = |host: &str, ips: &str| {
        let entry = hosts.entry(host.to_string()).or_default();
        for ip in ips.split(',') {
            let ip = ip.strip_prefix('[').and_then(|i| i.strip_suffix(']')).filter(|i| !i.is_empty()).unwrap_or(ip);
            entry.push(ip.to_string());
        }
    };
    match node.value {
        Value::Seq(items) => {
            for i in items {
                let s = go_sprint(&i);
                let split = s.split_once('=').or_else(|| s.split_once(':'));
                let Some((host, ips)) = split else {
                    return Err(Diagnostic::error("extra-hosts-invalid", format!("{p} : hôte supplémentaire invalide, IP manquante : {s}")).at(p, i.span));
                };
                add(host, ips);
            }
        }
        Value::Map(m) => {
            for (host, v) in m {
                match v.value {
                    Value::Seq(items) => {
                        for i in items {
                            add(&host, &go_sprint(&i));
                        }
                    }
                    Value::Null => add(&host, ""),
                    _ => add(&host, &go_sprint(&v)),
                }
            }
        }
        _ => return Err(Diagnostic::error("extra-hosts-invalid", format!("{p} : type inattendu pour extra_hosts")).at(p, span)),
    }
    for host in hosts.keys() {
        if host.is_empty() || host.contains([':', '=']) {
            return Err(Diagnostic::error("extra-hosts-invalid", format!("{p} : nom d'hôte invalide `{host}`")).at(p, span));
        }
    }
    let mut list: Vec<String> = hosts.iter().flat_map(|(h, ips)| ips.iter().map(move |ip| format!("{h}={ip}"))).collect();
    list.sort();
    Ok(Node::new(span, Value::Seq(list.into_iter().map(|s| str_node(span, s)).collect())))
}

fn shell_command(node: Node, p: &Path) -> Result<Node, Diagnostic> {
    match node.value {
        Value::Str(s) => {
            let words = shellwords(&s).map_err(|e| Diagnostic::error("command-invalid", format!("{p} : {e}")).at(p, node.span))?;
            Ok(Node::new(node.span, Value::Seq(words.into_iter().map(|w| str_node(node.span, w)).collect())))
        }
        _ => Ok(node),
    }
}

/// Port de `shellwords.Parse` (mattn/go-shellwords, options par défaut :
/// pas d'expansion, pas de backticks, pas de commentaires). Un `;`, `&`,
/// `|`, `<` ou `>` hors guillemets arrête l'analyse, comme dans l'original.
pub fn shellwords(line: &str) -> Result<Vec<String>, String> {
    let mut args: Vec<String> = Vec::new();
    let mut buf = String::new();
    let (mut escaped, mut dq, mut sq, mut dollar) = (false, false, false, false);
    // `got` : rien, un mot simple, ou un mot quoté (qui peut être vide).
    let mut got = 0u8;
    let flush = |buf: &mut String, got: &mut u8, args: &mut Vec<String>| {
        if *got == 2 || (*got != 0 && !buf.is_empty()) {
            args.push(std::mem::take(buf));
        }
        buf.clear();
        *got = 0;
    };
    let chars = line.chars();
    for r in chars {
        if escaped {
            escaped = false;
            let r = match r {
                't' if !dollar => '\t',
                'n' if !dollar => '\n',
                _ => r,
            };
            if dollar {
                buf.push('\\');
            }
            buf.push(r);
            got = 1;
            continue;
        }
        if r == '\\' {
            if sq {
                buf.push('\\');
            } else {
                escaped = true;
            }
            continue;
        }
        if matches!(r, ' ' | '\t' | '\r' | '\n') {
            if sq || dq || dollar {
                buf.push(r);
            } else {
                flush(&mut buf, &mut got, &mut args);
            }
            continue;
        }
        match r {
            '`' => {
                if !sq && !dq && !dollar {
                    return Err("les backticks ne sont pas supportés dans une commande".into());
                }
            }
            ')' => {
                if !sq && !dq {
                    if !dollar {
                        return Err("`)` inattendue".into());
                    }
                    buf.push(')');
                    dollar = false;
                    got = 1;
                    continue;
                }
            }
            '(' => {
                if !sq && !dq {
                    if !dollar && buf.ends_with('$') && !buf.ends_with("\\$") {
                        dollar = true;
                        buf.push('(');
                        continue;
                    }
                    return Err("`(` inattendue".into());
                }
            }
            '"' => {
                if !sq && !dollar {
                    if dq {
                        got = 2;
                    }
                    dq = !dq;
                    continue;
                }
            }
            '\'' => {
                if !dq && !dollar {
                    if sq {
                        got = 2;
                    }
                    sq = !sq;
                    continue;
                }
            }
            ';' | '&' | '|' | '<' | '>' if !(sq || dq || dollar) => break,
            _ => {}
        }
        got = 1;
        buf.push(r);
    }
    flush(&mut buf, &mut got, &mut args);
    if escaped || sq || dq || dollar {
        return Err("ligne de commande invalide (guillemet non fermé)".into());
    }
    Ok(args)
}

fn nano_cpus(node: Node, p: &Path) -> Result<Node, Diagnostic> {
    let f = match &node.value {
        Value::Int(i) => *i as f64,
        Value::Float(f) => *f,
        Value::Str(s) => s.trim().parse::<f64>().map_err(|_| Diagnostic::error("cpus-invalid", format!("{p} : nombre de CPU invalide `{s}`")).at(p, node.span))?,
        _ => return Ok(node),
    };
    Ok(Node { value: Value::Float(f), ..node })
}

/// `UnitBytes` : entier, ou taille lisible (`512m`, `1g`), rendu en chaîne
/// décimale d'octets comme `MarshalYAML`.
fn unit_bytes(node: Node, p: &Path) -> Result<Node, Diagnostic> {
    let bytes = match &node.value {
        Value::Int(i) => *i,
        Value::Float(f) => *f as i64,
        Value::Str(s) => parse_ram_in_bytes(s).ok_or_else(|| Diagnostic::error("bytes-invalid", format!("{p} : taille invalide `{s}`")).at(p, node.span))?,
        _ => return Ok(node),
    };
    Ok(str_node(node.span, bytes.to_string()))
}

/// `units.RAMInBytes` (docker/go-units) : nombre décimal (fractions
/// acceptées), suffixe optionnel `b`, `k`, `m`, `g`, `t`, `p` avec `b`,
/// `ib` ou `iB` facultatif, insensible à la casse, base 1024.
pub fn parse_ram_in_bytes(s: &str) -> Option<i64> {
    if let Ok(n) = s.parse::<i64>() {
        return Some(n);
    }
    let s = s.trim();
    let split = s.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))?;
    let (num, unit) = s.split_at(split);
    let value: f64 = num.parse().ok()?;
    let unit = unit.trim().to_lowercase();
    let unit = unit.strip_suffix("ib").or_else(|| unit.strip_suffix('b')).unwrap_or(&unit);
    let mult: f64 = match unit {
        "" => 1.0,
        "k" => 1024.0,
        "m" => 1024f64.powi(2),
        "g" => 1024f64.powi(3),
        "t" => 1024f64.powi(4),
        "p" => 1024f64.powi(5),
        _ => return None,
    };
    Some((value * mult) as i64)
}

/// `Duration` : lue par `str2duration.ParseDuration`, rendue par
/// `time.Duration.String()`.
fn duration(node: Node, p: &Path) -> Result<Node, Diagnostic> {
    let text = go_sprint(&node);
    let nanos = parse_duration(&text).ok_or_else(|| Diagnostic::error("duration-invalid", format!("{p} : durée invalide `{text}`")).at(p, node.span))?;
    Ok(str_node(node.span, go_duration_string(nanos)))
}

/// `str2duration.ParseDuration` : comme `time.ParseDuration` (`ns`, `us`,
/// `µs`, `ms`, `s`, `m`, `h`) plus `d` et `w`, fractions permises.
pub fn parse_duration(s: &str) -> Option<i128> {
    let (neg, mut rest) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    if rest == "0" {
        return Some(0);
    }
    if rest.is_empty() {
        return None;
    }
    let mut total: i128 = 0;
    while !rest.is_empty() {
        let end = rest.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(rest.len());
        let num = &rest[..end];
        if num.is_empty() || num == "." {
            return None;
        }
        let value: f64 = num.parse().ok()?;
        rest = &rest[end..];
        let uend = rest.find(|c: char| c.is_ascii_digit() || c == '.').unwrap_or(rest.len());
        let unit = &rest[..uend];
        rest = &rest[uend..];
        let mult: i128 = match unit {
            "ns" => 1,
            "us" | "µs" | "μs" => 1_000,
            "ms" => 1_000_000,
            "s" => 1_000_000_000,
            "m" => 60 * 1_000_000_000,
            "h" => 3600 * 1_000_000_000,
            "d" => 86400 * 1_000_000_000,
            "w" => 7 * 86400 * 1_000_000_000,
            _ => return None,
        };
        total += (value * mult as f64) as i128;
    }
    Some(if neg { -total } else { total })
}

/// `time.Duration.String()` : `1h0m0s`, `1m30s`, `500ms`, `1.5s`, `0s`.
pub fn go_duration_string(nanos: i128) -> String {
    if nanos == 0 {
        return "0s".into();
    }
    let neg = nanos < 0;
    let u = nanos.unsigned_abs();
    let mut out = String::new();
    if u < 1_000_000_000 {
        // Moins d'une seconde : ns, µs ou ms avec fraction.
        let (div, unit) = if u < 1_000 {
            (1u128, "ns")
        } else if u < 1_000_000 {
            (1_000, "µs")
        } else {
            (1_000_000, "ms")
        };
        out.push_str(&fmt_frac(u, div));
        out.push_str(unit);
    } else {
        let hours = u / 3_600_000_000_000;
        let rem = u % 3_600_000_000_000;
        let mins = rem / 60_000_000_000;
        let secs = rem % 60_000_000_000;
        if hours > 0 {
            out.push_str(&format!("{hours}h"));
        }
        if hours > 0 || mins > 0 {
            out.push_str(&format!("{mins}m"));
        }
        out.push_str(&fmt_frac(secs, 1_000_000_000));
        out.push('s');
    }
    if neg {
        out.insert(0, '-');
    }
    out
}

fn fmt_frac(v: u128, div: u128) -> String {
    let int = v / div;
    let frac = v % div;
    if frac == 0 {
        return int.to_string();
    }
    let width = div.to_string().len() - 1;
    let f = format!("{frac:0width$}");
    format!("{int}.{}", f.trim_end_matches('0'))
}


/// `omitempty` de la sérialisation Go : dans une structure, un champ à sa
/// valeur zéro (null, `false`, `0`, `""`, liste vide) disparaît. Les
/// mappings à valeurs libres (environnement, labels, extensions...) gardent
/// tout, et une structure vide reste (pointeur non nul côté Go :
/// `volume: {}`, `healthcheck: {}`).
/// `Project.WithoutUnnecessaryResources` : `docker compose config` retire
/// les réseaux, volumes, secrets et configs qu'aucun service n'utilise.
pub fn prune_unused(tree: &mut Node) {
    let Some(root) = tree.as_map_mut() else { return };
    let mut networks = Vec::new();
    let mut volumes = Vec::new();
    let mut secrets = Vec::new();
    let mut configs = Vec::new();
    if let Some(services) = root.get("services").and_then(Node::as_map) {
        for s in services.values().filter_map(Node::as_map) {
            if let Some(n) = s.get("networks").and_then(Node::as_map) {
                networks.extend(n.keys().cloned());
            }
            if let Some(v) = s.get("volumes").and_then(Node::as_seq) {
                for m in v.iter().filter_map(Node::as_map) {
                    if m.get("type").and_then(Node::as_str) == Some("volume") {
                        if let Some(src) = m.get("source").and_then(Node::as_str).filter(|s| !s.is_empty()) {
                            volumes.push(src.to_string());
                        }
                    }
                }
            }
            let sources = |key: &str| -> Vec<String> {
                s.get(key).and_then(Node::as_seq).map(|l| l.iter().filter_map(|m| m.as_map().and_then(|m| m.get("source")).and_then(Node::as_str).map(str::to_string)).collect()).unwrap_or_default()
            };
            secrets.extend(sources("secrets"));
            if let Some(b) = s.get("build").and_then(Node::as_map) {
                if let Some(l) = b.get("secrets").and_then(Node::as_seq) {
                    secrets.extend(l.iter().filter_map(|m| m.as_map().and_then(|m| m.get("source")).and_then(Node::as_str).map(str::to_string)));
                }
            }
            configs.extend(sources("configs"));
        }
    }
    for (key, used) in [("networks", networks), ("volumes", volumes), ("secrets", secrets), ("configs", configs)] {
        if let Some(m) = root.get_mut(key).and_then(Node::as_map_mut) {
            m.retain(|k, _| used.contains(k));
        }
        if root.get(key).and_then(Node::as_map).is_some_and(|m| m.is_empty()) {
            root.shift_remove(key);
        }
    }
}

pub fn omit_zero(node: Node) -> Node {
    omit_zero_at(node, &Path::root())
}

fn is_value_map(p: &Path) -> bool {
    const VALUE_MAPS: &[&str] = &[
        "services",
        "networks",
        "volumes",
        "secrets",
        "configs",
        "services.*.environment",
        "services.*.labels",
        "services.*.annotations",
        "services.*.sysctls",
        "services.*.extra_hosts",
        "services.*.networks",
        "services.*.depends_on",
        "services.*.build.args",
        "services.*.build.labels",
        "services.*.build.additional_contexts",
        "services.*.deploy.labels",
        "services.*.logging.options",
        "services.*.ulimits",
        "services.*.volumes.[].volume.labels",
        "networks.*.labels",
        "networks.*.driver_opts",
        "networks.*.ipam.options",
        "networks.*.ipam.config.[].aux_addresses",
        "volumes.*.labels",
        "volumes.*.driver_opts",
        "secrets.*.labels",
        "configs.*.labels",
    ];
    VALUE_MAPS.iter().any(|pat| p.matches(pat))
}

fn is_zero(n: &Node) -> bool {
    match &n.value {
        Value::Null => true,
        Value::Bool(b) => !b,
        Value::Int(i) => *i == 0,
        Value::Float(f) => *f == 0.0,
        Value::Str(s) => s.is_empty(),
        Value::Timestamp(_) => false,
        Value::Seq(s) => s.is_empty(),
        Value::Map(_) => false,
    }
}

fn omit_zero_at(node: Node, p: &Path) -> Node {
    match node.value {
        Value::Map(m) => {
            let value_map = is_value_map(p);
            let mut out = IndexMap::with_capacity(m.len());
            for (k, v) in m {
                // Les extensions `x-*` sont reproduites telles quelles.
                if k.starts_with("x-") {
                    out.insert(k, v);
                    continue;
                }
                let cp = p.child(&k);
                // `ServiceDependency.Required` n'a pas d'omitempty.
                let always = cp.matches("services.*.depends_on.*.required");
                if !value_map && !always && is_zero(&v) {
                    continue;
                }
                // `OptOut` : booléen dont la valeur zéro est `true`.
                if cp.matches("services.*.volumes.[].bind.create_host_path") && v.value == Value::Bool(true) {
                    continue;
                }
                out.insert(k, omit_zero_at(v, &cp));
            }
            Node { span: node.span, tag: node.tag, value: Value::Map(out) }
        }
        Value::Seq(s) => Node { span: node.span, tag: node.tag, value: Value::Seq(s.into_iter().enumerate().map(|(i, v)| omit_zero_at(v, &p.index(i))).collect()) },
        _ => node,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shellwords_basics() {
        assert_eq!(shellwords("echo yes").unwrap(), vec!["echo", "yes"]);
        assert_eq!(shellwords("sh -c 'a && b'").unwrap(), vec!["sh", "-c", "a && b"]);
        assert_eq!(shellwords(r#"x "a b" c\ d"#).unwrap(), vec!["x", "a b", "c d"]);
        assert_eq!(shellwords("a ''").unwrap(), vec!["a", ""]);
        assert_eq!(shellwords("--default-authentication-plugin=mysql_native_password").unwrap().len(), 1);
        assert_eq!(shellwords("a && b").unwrap(), vec!["a"]);
        assert!(shellwords("a 'b").is_err());
    }

    #[test]
    fn bytes_and_durations() {
        assert_eq!(parse_ram_in_bytes("512m"), Some(536870912));
        assert_eq!(parse_ram_in_bytes("1g"), Some(1073741824));
        assert_eq!(parse_ram_in_bytes("1.5k"), Some(1536));
        assert_eq!(parse_ram_in_bytes("100"), Some(100));
        assert_eq!(parse_ram_in_bytes("2GiB"), Some(2147483648));
        assert_eq!(parse_ram_in_bytes("x"), None);
        let d = |s: &str| go_duration_string(parse_duration(s).unwrap());
        assert_eq!(d("1m30s"), "1m30s");
        assert_eq!(d("90s"), "1m30s");
        assert_eq!(d("1h"), "1h0m0s");
        assert_eq!(d("500ms"), "500ms");
        assert_eq!(d("1.5s"), "1.5s");
        assert_eq!(d("0"), "0s");
        assert_eq!(d("2d"), "48h0m0s");
        assert!(parse_duration("abc").is_none());
    }
}
