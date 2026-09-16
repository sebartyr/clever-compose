//! Édition ciblée d'un fichier compose : poser des clés `x-clever` sous un
//! service sans réécrire le reste (commentaires, ordre, ancres restent).
//! Travaille sur le texte : repère `services:`, le service, son bloc
//! `x-clever:` (créé s'il manque), et remplace ou ajoute les clés. Refuse
//! ce qu'il ne reconnaît pas (indentation irrégulière, service absent)
//! plutôt que de produire un fichier faux.

/// Valeur YAML scalaire, déjà rendue (`M`, `"15"`, `3`, `true`).
pub struct Entry<'a> {
    pub key: &'a str,
    /// `None` retire la clé.
    pub value: Option<String>,
}

fn indent_of(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ').count()
}

fn is_blank_or_comment(line: &str) -> bool {
    let t = line.trim_start();
    t.is_empty() || t.starts_with('#')
}

/// Ligne `key:` (avec ou sans valeur) à l'indentation exacte `indent`.
fn key_at(line: &str, indent: usize, key: &str) -> bool {
    indent_of(line) == indent && {
        let t = &line[indent..];
        t.starts_with(key) && t[key.len()..].trim_start().starts_with(':') && (t[key.len()..].trim_start().len() == 1 || !t[key.len()..].trim_start()[1..].starts_with(|c: char| !c.is_whitespace()))
    }
}

/// Fin (exclusive) d'un bloc commençant à la ligne `start` (sa clé est à
/// l'indentation `indent`) : première ligne non vide d'indentation ≤ indent.
fn block_end(lines: &[&str], start: usize, indent: usize) -> usize {
    let mut i = start + 1;
    while i < lines.len() {
        if !is_blank_or_comment(lines[i]) && indent_of(lines[i]) <= indent {
            break;
        }
        i += 1;
    }
    // Les lignes vides de fin de bloc restent au bloc suivant.
    while i > start + 1 && is_blank_or_comment(lines[i - 1]) {
        i -= 1;
    }
    i
}

pub fn set_xclever(text: &str, service: &str, entries: &[Entry<'_>]) -> Result<String, String> {
    let lines: Vec<&str> = text.lines().collect();
    let services = lines.iter().position(|l| key_at(l, 0, "services")).ok_or("pas de `services:` à la racine")?;
    let services_end = block_end(&lines, services, 0);
    // Indentation des services : celle de la première clé du bloc.
    let svc_indent = lines[services + 1..services_end].iter().find(|l| !is_blank_or_comment(l)).map(|l| indent_of(l)).ok_or("`services:` est vide")?;
    let svc = (services + 1..services_end).find(|&i| key_at(lines[i], svc_indent, service)).ok_or_else(|| format!("service `{service}` introuvable dans ce fichier (défini dans un autre fichier ou par `extends` ?)"))?;
    let svc_end = block_end(&lines, svc, svc_indent);
    let child_indent = lines[svc + 1..svc_end].iter().find(|l| !is_blank_or_comment(l)).map(|l| indent_of(l)).unwrap_or(svc_indent + 2);
    if child_indent <= svc_indent {
        return Err(format!("service `{service}` sans contenu indenté"));
    }
    let step = child_indent - svc_indent;
    let mut out: Vec<String> = lines.iter().map(|l| l.to_string()).collect();
    let pad = |n: usize| " ".repeat(n);
    match (svc + 1..svc_end).find(|&i| key_at(lines[i], child_indent, "x-clever")) {
        Some(x) => {
            // `x-clever: {…}` en flux : pas de réécriture fiable.
            if lines[x][child_indent + "x-clever".len()..].trim_start().trim_start_matches(':').trim().starts_with('{') {
                return Err(format!("`x-clever` de `{service}` est écrit en une ligne (`{{…}}`) : passez-le en bloc pour l'édition"));
            }
            let x_end = block_end(&lines, x, child_indent);
            let key_indent = lines[x + 1..x_end].iter().find(|l| !is_blank_or_comment(l)).map(|l| indent_of(l)).unwrap_or(child_indent + step);
            // Du dernier au premier pour garder les index valides.
            let mut insertions: Vec<String> = Vec::new();
            let mut removals: Vec<usize> = Vec::new();
            for e in entries {
                match (x + 1..x_end).find(|&i| key_at(lines[i], key_indent, e.key)) {
                    Some(i) => match &e.value {
                        Some(v) => out[i] = format!("{}{}: {v}", pad(key_indent), e.key),
                        None => removals.push(i),
                    },
                    None => {
                        if let Some(v) = &e.value {
                            insertions.push(format!("{}{}: {v}", pad(key_indent), e.key));
                        }
                    }
                }
            }
            let removed_before_end = removals.iter().filter(|&&i| i < x_end).count();
            removals.sort_unstable_by(|a, b| b.cmp(a));
            for i in removals {
                out.remove(i);
            }
            let insert_at = x_end - removed_before_end;
            for (k, l) in insertions.into_iter().enumerate() {
                out.insert(insert_at + k, l);
            }
        }
        None => {
            let mut block = vec![format!("{}x-clever:", pad(child_indent))];
            for e in entries {
                if let Some(v) = &e.value {
                    block.push(format!("{}{}: {v}", pad(child_indent + step), e.key));
                }
            }
            if block.len() == 1 {
                return Ok(text.to_string());
            }
            for (k, l) in block.into_iter().enumerate() {
                out.insert(svc_end + k, l);
            }
        }
    }
    let mut s = out.join("\n");
    if text.ends_with('\n') {
        s.push('\n');
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "services:\n  web:\n    image: nginx\n    ports:\n      - \"8080:80\"\n  api:\n    image: api\n    x-clever:\n      flavor: S\n      task: true\nnetworks:\n  front: {}\n";

    #[test]
    fn adds_block_when_missing() {
        let out = set_xclever(SRC, "web", &[Entry { key: "min_flavor", value: Some("S".into()) }, Entry { key: "max_instances", value: Some("3".into()) }]).unwrap();
        assert!(out.contains("  web:\n    image: nginx\n    ports:\n      - \"8080:80\"\n    x-clever:\n      min_flavor: S\n      max_instances: 3\n  api:"), "{out}");
    }

    #[test]
    fn replaces_and_removes_in_existing_block() {
        let out = set_xclever(SRC, "api", &[Entry { key: "flavor", value: None }, Entry { key: "min_flavor", value: Some("M".into()) }, Entry { key: "task", value: Some("false".into()) }]).unwrap();
        assert!(out.contains("    x-clever:\n      task: false\n      min_flavor: M\nnetworks:"), "{out}");
        assert!(!out.contains("flavor: S"));
    }

    #[test]
    fn unknown_service() {
        assert!(set_xclever(SRC, "db", &[]).is_err());
    }
}
