//! Substitution de variables façon shell : `$VAR`, `${VAR}`, `${VAR:-def}`,
//! `${VAR-def}`, `${VAR:?err}`, `${VAR?err}`, `${VAR:+alt}`, `${VAR+alt}`,
//! et `$$` pour un `$` littéral.
//!
//! Port de `template/template.go` de compose-go, sans le regex : la
//! substitution est un balayage, avec les mêmes décisions que le moteur
//! original (choix de l'opérateur par première occurrence, valeur par
//! défaut elle-même substituée, `${` non fermé en erreur, `$` suivi d'autre
//! chose laissé tel quel).

use std::fmt;

/// Fonction de recherche d'une variable : `(valeur, définie)`.
pub type Lookup<'a> = &'a dyn Fn(&str) -> Option<String>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateError {
    /// `${` mal formé ; contient le modèle fautif.
    Invalid(String),
    /// `${VAR:?msg}` ou `${VAR?msg}` sans valeur.
    MissingRequired { variable: String, reason: String },
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TemplateError::Invalid(t) => write!(f, "format d'interpolation invalide ; échappez les `$` avec `$$` : {t}"),
            TemplateError::MissingRequired { variable, reason } if reason.is_empty() => write!(f, "la variable `{variable}` est requise"),
            TemplateError::MissingRequired { variable, reason } => write!(f, "la variable `{variable}` est requise : {reason}"),
        }
    }
}

/// Noms de variables reconnus : `[_a-zA-Z][_a-zA-Z0-9]*`.
fn name_len(s: &str) -> usize {
    let b = s.as_bytes();
    if b.is_empty() || !(b[0] == b'_' || b[0].is_ascii_alphabetic()) {
        return 0;
    }
    b.iter().take_while(|c| **c == b'_' || c.is_ascii_alphanumeric()).count()
}

/// `getFirstBraceClosingIndex` : index du `}` qui ferme le `{` initial de
/// `s` (qui commence par `{`), en sautant le caractère qui suit chaque `{`.
fn closing_brace(s: &[u8]) -> Option<usize> {
    let mut open = 0i32;
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'}' {
            open -= 1;
            if open == 0 {
                return Some(i);
            }
        }
        if s[i] == b'{' {
            open += 1;
            i += 1;
        }
        i += 1;
    }
    None
}

pub fn substitute(template: &str, lookup: Lookup<'_>) -> Result<String, TemplateError> {
    substitute_tracking(template, lookup, &mut Vec::new())
}

/// Comme [`substitute`], en notant dans `unset` les variables absentes
/// remplacées par une chaîne vide (`$X`, `${X}` sans opérateur) : Compose
/// avertit dans ce cas, le chargeur aussi.
pub fn substitute_tracking(template: &str, lookup: Lookup<'_>, unset: &mut Vec<String>) -> Result<String, TemplateError> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(pos) = rest.find('$') {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 1..];
        if let Some(r) = after.strip_prefix('$') {
            out.push('$');
            rest = r;
            continue;
        }
        if let Some(inner_start) = after.strip_prefix('{') {
            let Some(close) = closing_brace(after.as_bytes()) else {
                return Err(TemplateError::Invalid(template.to_string()));
            };
            let inner = &inner_start[..close - 1];
            let n = name_len(inner);
            if n == 0 {
                return Err(TemplateError::Invalid(template.to_string()));
            }
            let (name, op_and_arg) = inner.split_at(n);
            let value = if op_and_arg.is_empty() {
                lookup(name).unwrap_or_else(|| {
                    unset.push(name.to_string());
                    String::new()
                })
            } else {
                braced(name, op_and_arg, lookup, template)?
            };
            out.push_str(&value);
            rest = &after[close + 1..];
            continue;
        }
        let n = name_len(after);
        if n == 0 {
            // `$` isolé, `$1`, `$ ` : laissé tel quel.
            out.push('$');
            rest = after;
            continue;
        }
        let name = &after[..n];
        out.push_str(&lookup(name).unwrap_or_else(|| {
            unset.push(name.to_string());
            String::new()
        }));
        rest = &after[n..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Forme `${NAME<op><arg>}`. L'opérateur est celui qui apparaît en premier
/// dans `NAME<op><arg>` parmi `:?`, `?`, `:-`, `-`, `:+`, `+` (ordre de
/// priorité à position égale), comme `getSubstitutionFunctionForTemplate`.
fn braced(name: &str, op_and_arg: &str, lookup: Lookup<'_>, template: &str) -> Result<String, TemplateError> {
    let full = format!("{name}{op_and_arg}");
    let ops = [":?", "?", ":-", "-", ":+", "+"];
    let mut best: Option<(usize, &str)> = None;
    for op in ops {
        if let Some(i) = full.find(op) {
            if best.is_none_or(|(bi, _)| i < bi) {
                best = Some((i, op));
            }
        }
    }
    let Some((_, op)) = best else {
        // Le regex original n'aurait pas reconnu `${NAME<autre chose>}`.
        return Err(TemplateError::Invalid(template.to_string()));
    };
    let (var, arg) = full.split_once(op).unwrap();
    // L'opérateur peut apparaître avant la fin du nom (`${A-B-c}` : `-` en
    // premier) : le nom effectif est ce qui précède.
    let value = lookup(var);
    let subst_arg = || substitute(arg, lookup);
    Ok(match op {
        ":?" => match value {
            Some(v) if !v.is_empty() => v,
            _ => return Err(TemplateError::MissingRequired { variable: var.to_string(), reason: subst_arg()? }),
        },
        "?" => match value {
            Some(v) => v,
            None => return Err(TemplateError::MissingRequired { variable: var.to_string(), reason: subst_arg()? }),
        },
        ":-" => match value {
            Some(v) if !v.is_empty() => v,
            _ => subst_arg()?,
        },
        "-" => match value {
            Some(v) => v,
            None => subst_arg()?,
        },
        ":+" => match value {
            Some(v) if !v.is_empty() => subst_arg()?,
            Some(v) => v,
            None => String::new(),
        },
        "+" => match value {
            Some(_) => subst_arg()?,
            None => String::new(),
        },
        _ => unreachable!(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(k: &str) -> Option<String> {
        match k {
            "FOO" => Some("foo".into()),
            "EMPTY" => Some(String::new()),
            _ => None,
        }
    }

    fn s(t: &str) -> Result<String, TemplateError> {
        substitute(t, &env)
    }

    #[test]
    fn forms() {
        assert_eq!(s("a $FOO b").unwrap(), "a foo b");
        assert_eq!(s("${FOO}").unwrap(), "foo");
        assert_eq!(s("$$FOO").unwrap(), "$FOO");
        assert_eq!(s("$1 $ x").unwrap(), "$1 $ x");
        assert_eq!(s("${MISSING}").unwrap(), "");
        assert_eq!(s("${MISSING:-def}").unwrap(), "def");
        assert_eq!(s("${EMPTY:-def}").unwrap(), "def");
        assert_eq!(s("${EMPTY-def}").unwrap(), "");
        assert_eq!(s("${MISSING-def}").unwrap(), "def");
        assert_eq!(s("${FOO:+alt}").unwrap(), "alt");
        assert_eq!(s("${EMPTY:+alt}").unwrap(), "");
        assert_eq!(s("${EMPTY+alt}").unwrap(), "alt");
        assert_eq!(s("${MISSING+alt}").unwrap(), "");
        assert_eq!(s("${MISSING:-${FOO}x}").unwrap(), "foox");
        assert_eq!(s("${MISSING:-a}${FOO}").unwrap(), "afoo");
        assert_eq!(s("x${MISSING:-}y").unwrap(), "xy");
        assert!(matches!(s("${MISSING:?need}"), Err(TemplateError::MissingRequired { reason, .. }) if reason == "need"));
        assert!(matches!(s("${EMPTY?}"), Ok(v) if v.is_empty()));
        assert!(matches!(s("${FOO"), Err(TemplateError::Invalid(_))));
        assert!(matches!(s("${}"), Err(TemplateError::Invalid(_))));
        assert!(matches!(s("${1X}"), Err(TemplateError::Invalid(_))));
    }
}
