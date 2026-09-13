//! Rendu HCL lisible depuis la structure JSON de l'émetteur Terraform
//! (CLAUDE.md §10 : une seule structure, validée en JSON, rendue par
//! templating). Le schéma du provider dit ce qui est un bloc (`deployment`)
//! et ce qui est un attribut imbriqué (`networkgroups = [...]`).

use serde_json::Value as J;
use std::fmt::Write;

pub fn render(tf: &J, schema: &J) -> String {
    let mut out = String::new();
    if let Some(t) = tf.get("terraform") {
        out.push_str("terraform {\n");
        if let Some(rp) = t.get("required_providers").and_then(J::as_object) {
            out.push_str("  required_providers {\n");
            for (k, v) in rp {
                let _ = writeln!(out, "    {k} = {}", value(v, 2, true));
            }
            out.push_str("  }\n");
        }
        out.push_str("}\n\n");
    }
    if let Some(p) = tf.get("provider").and_then(J::as_object) {
        for (name, body) in p {
            let _ = writeln!(out, "provider \"{name}\" {{");
            attrs(&mut out, body, 1, &[], None);
            out.push_str("}\n\n");
        }
    }
    if let Some(vars) = tf.get("variable").and_then(J::as_object) {
        for (name, body) in vars {
            let _ = writeln!(out, "variable \"{name}\" {{");
            // `type` est une expression de type, nue.
            let mut b = body.clone();
            if let Some(t) = b.get("type").and_then(J::as_str).map(str::to_string) {
                b["type"] = J::String(format!("${{{t}}}"));
            }
            attrs(&mut out, &b, 1, &[], None);
            out.push_str("}\n\n");
        }
    }
    if let Some(res) = tf.get("resource").and_then(J::as_object) {
        for (rtype, instances) in res {
            let blocks: Vec<String> = schema["provider_schemas"]["registry.terraform.io/clevercloud/clevercloud"]["resource_schemas"][rtype]["block"]["block_types"]
                .as_object()
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();
            for (name, body) in instances.as_object().into_iter().flatten() {
                let _ = writeln!(out, "resource \"{rtype}\" \"{name}\" {{");
                attrs(&mut out, body, 1, &blocks, Some("depends_on"));
                out.push_str("}\n\n");
            }
        }
    }
    out.trim_end().to_string() + "\n"
}

fn attrs(out: &mut String, body: &J, depth: usize, blocks: &[String], raw_list_key: Option<&str>) {
    let pad = "  ".repeat(depth);
    let Some(m) = body.as_object() else { return };
    let width = m.keys().filter(|k| !blocks.contains(k)).map(String::len).max().unwrap_or(0);
    for (k, v) in m {
        if blocks.contains(k) {
            let _ = writeln!(out, "{pad}{k} {{");
            attrs(out, v, depth + 1, &[], None);
            let _ = writeln!(out, "{pad}}}");
            continue;
        }
        if Some(k.as_str()) == raw_list_key {
            // `depends_on = [clevercloud_docker.db]` : références nues.
            let refs: Vec<String> = v.as_array().into_iter().flatten().filter_map(J::as_str).map(str::to_string).collect();
            let _ = writeln!(out, "{pad}{k:<width$} = [{}]", refs.join(", "));
            continue;
        }
        let _ = writeln!(out, "{pad}{k:<width$} = {}", value(v, depth, false));
    }
}

fn value(v: &J, depth: usize, compact: bool) -> String {
    let pad = "  ".repeat(depth + 1);
    let end = "  ".repeat(depth);
    match v {
        J::Null => "null".into(),
        J::Bool(b) => b.to_string(),
        J::Number(n) => n.to_string(),
        // Une chaîne réduite à une seule interpolation est une expression.
        J::String(s) => match single_interpolation(s) {
            Some(expr) => expr.to_string(),
            None => quote(s),
        },
        J::Array(a) => {
            if a.is_empty() {
                return "[]".into();
            }
            if compact || a.iter().all(|x| !x.is_object() && !x.is_array()) && a.len() <= 3 {
                return format!("[{}]", a.iter().map(|x| value(x, depth, true)).collect::<Vec<_>>().join(", "));
            }
            let items: Vec<String> = a.iter().map(|x| format!("{pad}{},", value(x, depth + 1, false))).collect();
            format!("[\n{}\n{end}]", items.join("\n"))
        }
        J::Object(m) => {
            if m.is_empty() {
                return "{}".into();
            }
            if compact {
                return format!("{{ {} }}", m.iter().map(|(k, x)| format!("{} = {}", key(k), value(x, depth, true))).collect::<Vec<_>>().join(", "));
            }
            let width = m.keys().map(|k| key(k).len()).max().unwrap_or(0);
            let items: Vec<String> = m.iter().map(|(k, x)| format!("{pad}{:<width$} = {}", key(k), value(x, depth + 1, false))).collect();
            format!("{{\n{}\n{end}}}", items.join("\n"))
        }
    }
}

fn key(k: &str) -> String {
    if k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !k.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        k.to_string()
    } else {
        quote(k)
    }
}

/// Chaîne HCL : les `${` et `%{` d'interpolation sont déjà voulus tels
/// quels (mêmes règles d'échappement qu'en JSON Terraform).
fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `${expr}` seul, accolades équilibrées (interpolations imbriquées et
/// chaînes internes comprises) : renvoie `expr`.
fn single_interpolation(s: &str) -> Option<&str> {
    let body = s.strip_prefix("${")?;
    let b = body.as_bytes();
    let (mut depth, mut in_str, mut i) = (1usize, false, 0usize);
    while i < b.len() {
        match b[i] {
            b'\\' if in_str => i += 1,
            b'"' => in_str = !in_str,
            b'{' if !in_str => depth += 1,
            b'}' if !in_str => {
                depth -= 1;
                if depth == 0 {
                    return if i == b.len() - 1 { Some(&body[..i]) } else { None };
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}
