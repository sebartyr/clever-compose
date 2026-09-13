//! Fichiers `.env` et `env_file`. Port de `dotenv/parser.go` de compose-go
//! (fork de godotenv) : `KEY=value`, `export KEY=value`, `KEY` seul hérité
//! de l'environnement, guillemets simples littéraux, guillemets doubles avec
//! échappements et substitution, valeurs nues avec substitution et
//! commentaire ` #` en fin de ligne.

use indexmap::IndexMap;

use super::template::{self, Lookup};

pub type Vars = IndexMap<String, String>;

/// Analyse `src`, ajoute les variables à `out`. `lookup` fournit
/// l'environnement pour l'héritage (`KEY` seul) et la substitution, avec
/// priorité sur les variables déjà lues dans le fichier.
pub fn parse(src: &str, out: &mut Vars, lookup: Lookup<'_>) -> Result<(), String> {
    let mut line = 1usize;
    let mut cutset: &str = src;
    loop {
        cutset = statement_start(cutset, &mut line);
        if cutset.is_empty() {
            break;
        }
        let (key, left, inherited) = locate_key(cutset, &mut line)?;
        if key.contains(' ') {
            return Err(format!("ligne {line} : une clé ne peut pas contenir d'espace"));
        }
        if inherited {
            if let Some(v) = lookup(&key) {
                out.insert(key, v);
            }
            cutset = left;
            continue;
        }
        let (value, left) = extract_value(left, out, lookup, &mut line)?;
        out.insert(key, value);
        cutset = left;
    }
    Ok(())
}

fn is_space(c: char) -> bool {
    matches!(c, '\t' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{85}' | '\u{A0}')
}

fn statement_start<'a>(src: &'a str, line: &mut usize) -> &'a str {
    let Some(pos) = src.find(|c: char| !is_space(c) && c != '\n') else {
        return "";
    };
    *line += src[..pos].matches('\n').count();
    let src = &src[pos..];
    if !src.starts_with('#') {
        return src;
    }
    match src.find('\n') {
        Some(p) => statement_start(&src[p..], line),
        None => "",
    }
}

fn locate_key<'a>(src: &'a str, line: &mut usize) -> Result<(String, &'a str, bool), String> {
    let src = match src.strip_prefix("export") {
        Some(r) if r.starts_with(is_space) => r.trim_start_matches(is_space),
        _ => src,
    };
    let mut key = "";
    let mut offset = src.len();
    let mut inherited = false;
    let mut found = false;
    for (i, c) in src.char_indices() {
        if is_space(c) {
            continue;
        }
        match c {
            '=' | ':' | '\n' => {
                key = &src[..i];
                offset = i + 1;
                inherited = c == '\n';
                found = true;
                break;
            }
            '_' | '.' | '-' | '[' | ']' => {}
            c if c.is_alphanumeric() => {}
            c => {
                return Err(format!("ligne {line} : caractère inattendu {c:?} dans le nom de variable {:?}", src.lines().next().unwrap_or("")));
            }
        }
    }
    if !found {
        // Fin de fichier sans `=` : la clé est tout ce qui reste, héritée.
        key = src;
        inherited = true;
    }
    if inherited {
        *line += 1;
    }
    let key = key.trim_end().to_string();
    let left = src[offset.min(src.len())..].trim_start_matches(is_space);
    Ok((key, left, inherited))
}

fn extract_value<'a>(src: &'a str, vars: &Vars, lookup: Lookup<'_>, line: &mut usize) -> Result<(String, &'a str), String> {
    let quote = match src.as_bytes().first() {
        Some(b'"') => Some('"'),
        Some(b'\'') => Some('\''),
        _ => None,
    };
    let Some(quote) = quote else {
        let (value, rest) = src.split_once('\n').unwrap_or((src, ""));
        *line += 1;
        let value = value.split(" #").next().unwrap_or("").trim_end();
        return Ok((expand(value, vars, lookup)?, rest));
    };
    let mut chars: Vec<char> = Vec::new();
    let mut prev_escape = false;
    let body = &src[1..];
    for (i, c) in body.char_indices() {
        if c == '\n' {
            *line += 1;
        }
        if c != quote {
            if !prev_escape && c == '\\' {
                prev_escape = true;
                continue;
            }
            if prev_escape {
                prev_escape = false;
                chars.push('\\');
            }
            chars.push(c);
            continue;
        }
        if prev_escape {
            prev_escape = false;
            chars.push(c);
            continue;
        }
        let mut value: String = chars.into_iter().collect();
        if quote == '"' {
            value = expand(&expand_escapes(&value), vars, lookup)?;
        }
        return Ok((value, &body[i + c.len_utf8()..]));
    }
    let end = src.find('\n').unwrap_or(src.len());
    Err(format!("ligne {line} : valeur entre guillemets non terminée {}", &src[..end]))
}

/// `expandEscapes` : `\a \b \c \f \n \r \t \v \" \\`, `\0NNN` octal, et
/// `\$` devient `$$` pour rester littéral à la substitution.
fn expand_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('$') => {
                chars.next();
                out.push_str("$$");
            }
            Some('0') => {
                chars.next();
                let mut digits = String::new();
                while digits.len() < 3 && chars.peek().is_some_and(|d| d.is_ascii_digit()) {
                    digits.push(chars.next().unwrap());
                }
                match u32::from_str_radix(&digits, 8).ok().and_then(char::from_u32) {
                    Some(ch) if !digits.is_empty() => out.push(ch),
                    _ => out.push('\0'),
                }
            }
            Some(e @ ('a' | 'b' | 'c' | 'f' | 'n' | 'r' | 't' | 'v' | '"' | '\\')) => {
                chars.next();
                out.push(match e {
                    'a' => '\u{07}',
                    'b' => '\u{08}',
                    'c' => '\\',
                    'f' => '\u{0C}',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    'v' => '\u{0B}',
                    other => other,
                });
                if e == 'c' {
                    out.push('c');
                }
            }
            _ => out.push('\\'),
        }
    }
    out
}

fn expand(value: &str, vars: &Vars, lookup: Lookup<'_>) -> Result<String, String> {
    let combined = |k: &str| lookup(k).or_else(|| vars.get(k).cloned());
    template::substitute(value, &combined).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_like_compose_go() {
        let src = "# commentaire\nA=1\nexport B=two words # note\nC='lit $A'\nD=\"dq $A\\n\"\nE=${A}x\nF\nG=\n";
        let env = |k: &str| if k == "F" { Some("inherited".to_string()) } else { None };
        let mut out = Vars::new();
        parse(src, &mut out, &env).unwrap();
        assert_eq!(out["A"], "1");
        assert_eq!(out["B"], "two words");
        assert_eq!(out["C"], "lit $A");
        assert_eq!(out["D"], "dq 1\n");
        assert_eq!(out["E"], "1x");
        assert_eq!(out["F"], "inherited");
        assert_eq!(out["G"], "");
        assert!(parse("A B=1\n", &mut Vars::new(), &env).is_err());
        assert!(parse("A=\"open\n", &mut Vars::new(), &env).is_err());
    }
}
