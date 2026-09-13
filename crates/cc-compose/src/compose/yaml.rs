//! Chargement d'un document YAML en arbre brut.
//!
//! Le parseur (`saphyr`, YAML 1.2) est utilisé sans résolution des scalaires
//! (`early_parse(false)`) : la résolution est refaite ici pour reproduire
//! celle de la référence Go. Compose utilise `go.yaml.in/yaml/v4`, dont le
//! `resolve` (`internal/libyaml/resolver.go`) est ni 1.1 ni 1.2 :
//!
//! - `true`/`false`/`null`/`~`/`.inf`/`.nan` reconnus dans une casse limitée
//!   (`true`, `True`, `TRUE`) ; `yes`/`no`/`on`/`off` restent des chaînes ;
//! - entiers via `ParseInt(base 0)` après suppression des `_` : `0x`, `0o`,
//!   `0b`, et **octal à zéro initial** (`0777` → 511, `012` → 10) ;
//! - flottants sur le motif `^[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?$` ;
//! - pas de sexagésimal (`22:22` reste une chaîne) ;
//! - horodatages reconnus sur les scalaires **non quotés** commençant par
//!   `YYYY-`, quatre formats ;
//! - un scalaire quoté est toujours une chaîne.
//!
//! Vérifié contre `docker compose config` 5.5.1 le 13/09/2026 (fixture
//! `tests/yaml/resolution.yaml`).
//!
//! Les clés de fusion `<<` sont appliquées ici (le parseur ne le fait pas) et
//! les clés de mapping deviennent des chaînes (compose-go, phase 2).

use indexmap::IndexMap;
use saphyr::{MarkedYamlOwned, ScalarStyle, YamlDataOwned, YamlLoader};
use saphyr_parser::Parser;
use thiserror::Error;

use super::node::{Node, Span, Value};

#[derive(Debug, Error)]
pub enum YamlError {
    #[error("YAML invalide : {0}")]
    Scan(#[from] saphyr::ScanError),
    #[error("ligne {line}, colonne {col} : {message}")]
    Structure { line: usize, col: usize, message: String },
    #[error("le fichier contient {0} documents YAML, un seul est attendu")]
    MultipleDocuments(usize),
}

/// Charge un fichier YAML en arbre brut. `file` est l'index du fichier dans
/// la table du projet, reporté dans chaque [`Span`].
pub fn load(source: &str, file: usize) -> Result<Node, YamlError> {
    let mut parser = Parser::new_from_str(source);
    let mut loader: YamlLoader<'_, MarkedYamlOwned> = YamlLoader::default();
    loader.early_parse(false);
    parser.load(&mut loader, true)?;
    let mut docs = loader.into_documents();
    // Un document vide se charge en BadValue : équivaut à un fichier vide.
    docs.retain(|d| !matches!(d.data, YamlDataOwned::BadValue));
    match docs.len() {
        0 => Ok(Node::new(Span { file, line: 1, col: 1 }, Value::Map(IndexMap::new()))),
        1 => convert(docs.pop().unwrap(), file),
        n => Err(YamlError::MultipleDocuments(n)),
    }
}

fn span_of(y: &MarkedYamlOwned, file: usize) -> Span {
    Span { file, line: y.span.start.line(), col: y.span.start.col() + 1 }
}

fn err(span: Span, message: impl Into<String>) -> YamlError {
    YamlError::Structure { line: span.line, col: span.col, message: message.into() }
}

fn convert(y: MarkedYamlOwned, file: usize) -> Result<Node, YamlError> {
    let span = span_of(&y, file);
    match y.data {
        YamlDataOwned::Representation(text, style, tag) => {
            let mut node = Node::new(span, Value::Null);
            match tag {
                Some(t) if t.is_yaml_core_schema() => {
                    node.value = resolve_tagged(&text, &t.suffix, span)?;
                }
                Some(t) => {
                    // Tag applicatif (`!reset`, `!override`) : le scalaire se
                    // résout comme s'il n'était pas tagué.
                    node.value = resolve_styled(&text, style);
                    node.tag = Some(format!("{}{}", t.handle, t.suffix));
                }
                None => node.value = resolve_styled(&text, style),
            }
            Ok(node)
        }
        YamlDataOwned::Value(s) => Ok(Node::new(span, from_scalar(s))),
        YamlDataOwned::Sequence(seq) => {
            let items = seq.into_iter().map(|i| convert(i, file)).collect::<Result<_, _>>()?;
            Ok(Node::new(span, Value::Seq(items)))
        }
        YamlDataOwned::Mapping(map) => {
            let mut out: IndexMap<String, Node> = IndexMap::new();
            let mut merged: Vec<Node> = Vec::new();
            for (k, v) in map {
                let kspan = span_of(&k, file);
                let key = convert(k, file)?;
                if key.tag.is_none() && key.as_str() == Some("<<") {
                    merged.push(convert(v, file)?);
                    continue;
                }
                let key = stringify_key(&key).ok_or_else(|| err(kspan, "clé de mapping invalide"))?;
                if out.contains_key(&key) {
                    return Err(err(kspan, format!("clé `{key}` dupliquée")));
                }
                out.insert(key, convert(v, file)?);
            }
            // Clés de fusion : les clés explicites gagnent, puis la première
            // source de fusion, comme yaml.v3.
            for m in merged {
                let sources: Vec<Node> = match m.value {
                    Value::Map(_) => vec![m],
                    Value::Seq(items) => items,
                    _ => return Err(err(m.span, "`<<` attend un mapping ou une liste de mappings")),
                };
                for src in sources {
                    let Value::Map(srcmap) = src.value else {
                        return Err(err(src.span, "`<<` attend un mapping ou une liste de mappings"));
                    };
                    for (k, v) in srcmap {
                        out.entry(k).or_insert(v);
                    }
                }
            }
            Ok(Node::new(span, Value::Map(out)))
        }
        YamlDataOwned::Tagged(tag, inner) => {
            let mut node = convert(*inner, file)?;
            node.tag = Some(format!("{}{}", tag.handle, tag.suffix));
            Ok(node)
        }
        YamlDataOwned::Alias(_) => Err(err(span, "alias vers une ancre inconnue")),
        YamlDataOwned::BadValue => Err(err(span, "valeur YAML invalide")),
    }
}

fn from_scalar(s: saphyr::ScalarOwned) -> Value {
    use saphyr::ScalarOwned as S;
    match s {
        S::Null => Value::Null,
        S::Boolean(b) => Value::Bool(b),
        S::Integer(i) => Value::Int(i),
        S::FloatingPoint(f) => Value::Float(f.into_inner()),
        S::String(st) => Value::Str(st),
    }
}

/// Clé de mapping : toujours une chaîne (compose-go, phase 2).
fn stringify_key(key: &Node) -> Option<String> {
    Some(match &key.value {
        Value::Str(s) | Value::Timestamp(s) => s.clone(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        Value::Seq(_) | Value::Map(_) => return None,
    })
}

fn resolve_styled(text: &str, style: ScalarStyle) -> Value {
    match style {
        ScalarStyle::Plain => resolve_plain(text),
        _ => Value::Str(text.to_string()),
    }
}

/// Résolution d'un scalaire explicitement tagué avec le schéma core
/// (`!!str 12`, `!!int "12"`), comme `resolve` de yaml.v4 avec un tag.
fn resolve_tagged(text: &str, suffix: &str, span: Span) -> Result<Value, YamlError> {
    let plain = resolve_plain(text);
    let bad = || err(span, format!("`{text}` ne peut pas être lu comme !!{suffix}"));
    Ok(match suffix {
        "str" | "binary" => Value::Str(text.to_string()),
        "int" => match plain {
            v @ Value::Int(_) => v,
            _ => return Err(bad()),
        },
        "float" => match plain {
            Value::Int(i) => Value::Float(i as f64),
            v @ Value::Float(_) => v,
            _ => return Err(bad()),
        },
        "bool" => match plain {
            v @ Value::Bool(_) => v,
            _ => return Err(bad()),
        },
        "null" => match plain {
            Value::Null => Value::Null,
            _ => return Err(bad()),
        },
        "timestamp" => match plain {
            v @ Value::Timestamp(_) => v,
            _ => return Err(bad()),
        },
        // Tag core inconnu : chaîne.
        _ => Value::Str(text.to_string()),
    })
}

/// Port de `resolve` (yaml.v4, `internal/libyaml/resolver.go`) pour un
/// scalaire non quoté et non tagué.
pub fn resolve_plain(text: &str) -> Value {
    // Table fixe, consultée quel que soit le premier caractère.
    match text {
        "true" | "True" | "TRUE" => return Value::Bool(true),
        "false" | "False" | "FALSE" => return Value::Bool(false),
        "" | "~" | "null" | "Null" | "NULL" => return Value::Null,
        ".nan" | ".NaN" | ".NAN" => return Value::Float(f64::NAN),
        ".inf" | ".Inf" | ".INF" | "+.inf" | "+.Inf" | "+.INF" => return Value::Float(f64::INFINITY),
        "-.inf" | "-.Inf" | "-.INF" => return Value::Float(f64::NEG_INFINITY),
        "-0" | "-0.0" => return Value::Float(-0.0),
        _ => {}
    }
    let first = text.as_bytes()[0];
    match first {
        b'.' => {
            if let Ok(f) = text.parse::<f64>() {
                return Value::Float(f);
            }
        }
        b'0'..=b'9' | b'+' | b'-' => {
            if let Some(ts) = parse_timestamp(text) {
                return Value::Timestamp(ts);
            }
            let plain: String = text.chars().filter(|c| *c != '_').collect();
            if let Some(i) = parse_int_base0(&plain) {
                return Value::Int(i);
            }
            if is_yaml_float(&plain) {
                if let Ok(f) = plain.parse::<f64>() {
                    return Value::Float(f);
                }
            }
        }
        _ => {}
    }
    Value::Str(text.to_string())
}

/// `strconv.ParseInt(s, 0, 64)` puis `ParseUint` : signe optionnel, préfixes
/// `0x`, `0o`, `0b`, et zéro initial pour l'octal. Les `_` ont déjà été
/// retirés. Un entier non signé au-delà de i64 est rabattu sur i64::MAX
/// faute de type dédié, cas sans usage en Compose.
fn parse_int_base0(s: &str) -> Option<i64> {
    let (neg, body) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    if body.is_empty() {
        return None;
    }
    let (radix, digits) = if body.len() > 1 && body.starts_with('0') {
        match body.as_bytes()[1] {
            b'x' | b'X' => (16, &body[2..]),
            b'o' | b'O' => (8, &body[2..]),
            b'b' | b'B' => (2, &body[2..]),
            _ => (8, &body[1..]),
        }
    } else {
        (10, body)
    };
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    let magnitude = u64::from_str_radix(digits, radix).ok()?;
    if neg {
        i64::try_from(magnitude).ok().map(|m| -m).or(if magnitude == 1u64 << 63 { Some(i64::MIN) } else { None })
    } else {
        Some(i64::try_from(magnitude).unwrap_or(i64::MAX))
    }
}

/// `^[-+]?(\.[0-9]+|[0-9]+(\.[0-9]*)?)([eE][-+]?[0-9]+)?$`
fn is_yaml_float(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
        i += 1;
    }
    let digits = |i: &mut usize| {
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
        }
        *i - start
    };
    if i < b.len() && b[i] == b'.' {
        i += 1;
        if digits(&mut i) == 0 {
            return false;
        }
    } else {
        if digits(&mut i) == 0 {
            return false;
        }
        if i < b.len() && b[i] == b'.' {
            i += 1;
            digits(&mut i);
        }
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        i += 1;
        if i < b.len() && (b[i] == b'-' || b[i] == b'+') {
            i += 1;
        }
        if digits(&mut i) == 0 {
            return false;
        }
    }
    i == b.len()
}

/// `parseTimestamp` de yaml.v4 : `YYYY-M-D`, puis optionnellement `T` ou
/// `t` ou espace, `H:M:S` avec fraction, puis `Z` ou `±HH:MM` (seulement
/// après `T`). Sortie normalisée en RFC 3339, ce que compose-go émet.
fn parse_timestamp(s: &str) -> Option<String> {
    let b = s.as_bytes();
    // Vérification rapide de yaml.v4 : quatre chiffres puis `-`.
    if b.len() < 5 || !b[..4].iter().all(u8::is_ascii_digit) || b[4] != b'-' {
        return None;
    }
    let mut i = 0;
    let num = |i: &mut usize, min: usize, max: usize| -> Option<u32> {
        let start = *i;
        while *i < b.len() && b[*i].is_ascii_digit() && *i - start < max {
            *i += 1;
        }
        if *i - start < min {
            return None;
        }
        s[start..*i].parse().ok()
    };
    let expect = |i: &mut usize, c: u8| -> Option<()> {
        if *i < b.len() && b[*i] == c {
            *i += 1;
            Some(())
        } else {
            None
        }
    };
    let year = num(&mut i, 4, 4)?;
    expect(&mut i, b'-')?;
    let month = num(&mut i, 1, 2)?;
    expect(&mut i, b'-')?;
    let day = num(&mut i, 1, 2)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if i == b.len() {
        return Some(format!("{year:04}-{month:02}-{day:02}T00:00:00Z"));
    }
    let sep = b[i];
    if sep != b'T' && sep != b't' && sep != b' ' {
        return None;
    }
    i += 1;
    let hour = num(&mut i, 1, 2)?;
    expect(&mut i, b':')?;
    let min = num(&mut i, 1, 2)?;
    expect(&mut i, b':')?;
    let sec = num(&mut i, 1, 2)?;
    if hour > 23 || min > 59 || sec > 60 {
        return None;
    }
    let mut frac = String::new();
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i == start {
            return None;
        }
        frac = s[start..i].trim_end_matches('0').to_string();
    }
    let mut tz = "Z".to_string();
    if sep == b' ' {
        if i != b.len() {
            return None;
        }
    } else if i < b.len() {
        if b[i] == b'Z' {
            i += 1;
        } else if b[i] == b'+' || b[i] == b'-' {
            let sign = b[i] as char;
            i += 1;
            let th = num(&mut i, 2, 2)?;
            expect(&mut i, b':')?;
            let tm = num(&mut i, 2, 2)?;
            tz = format!("{sign}{th:02}:{tm:02}");
        } else {
            return None;
        }
        if i != b.len() {
            return None;
        }
    }
    let frac = if frac.is_empty() { String::new() } else { format!(".{frac}") };
    Some(format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}{frac}{tz}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Value {
        resolve_plain(s)
    }

    #[test]
    fn bools_and_nulls_match_yaml_v4() {
        assert_eq!(v("true"), Value::Bool(true));
        assert_eq!(v("TRUE"), Value::Bool(true));
        assert_eq!(v("yes"), Value::Str("yes".into()));
        assert_eq!(v("on"), Value::Str("on".into()));
        assert_eq!(v("y"), Value::Str("y".into()));
        assert_eq!(v("~"), Value::Null);
        assert_eq!(v(""), Value::Null);
        assert_eq!(v("NULL"), Value::Null);
    }

    #[test]
    fn ints_follow_parseint_base0() {
        assert_eq!(v("0777"), Value::Int(511));
        assert_eq!(v("012"), Value::Int(10));
        assert_eq!(v("0o777"), Value::Int(511));
        assert_eq!(v("0x1F"), Value::Int(31));
        assert_eq!(v("0b101"), Value::Int(5));
        assert_eq!(v("1_000"), Value::Int(1000));
        assert_eq!(v("+12"), Value::Int(12));
        assert_eq!(v("-7"), Value::Int(-7));
        assert_eq!(v("0"), Value::Int(0));
        // Octal invalide, mais le motif flottant l'accepte : yaml.v4 donne 9.0.
        assert_eq!(v("09"), Value::Float(9.0));
        assert_eq!(v("0_7_7"), Value::Int(63));
        assert_eq!(v("22:22"), Value::Str("22:22".into()));
        assert_eq!(v("1:00:00"), Value::Str("1:00:00".into()));
    }

    #[test]
    fn floats() {
        assert_eq!(v("1e3"), Value::Float(1000.0));
        assert_eq!(v(".5"), Value::Float(0.5));
        assert_eq!(v("0.5"), Value::Float(0.5));
        assert_eq!(v("1."), Value::Float(1.0));
        assert_eq!(v("1.2.3"), Value::Str("1.2.3".into()));
        assert_eq!(v("512m"), Value::Str("512m".into()));
        assert!(matches!(v(".inf"), Value::Float(f) if f.is_infinite()));
    }

    #[test]
    fn timestamps() {
        assert_eq!(v("2026-09-13"), Value::Timestamp("2026-09-13T00:00:00Z".into()));
        assert_eq!(v("2026-9-3"), Value::Timestamp("2026-09-03T00:00:00Z".into()));
        assert_eq!(
            v("2026-09-13T10:11:12Z"),
            Value::Timestamp("2026-09-13T10:11:12Z".into())
        );
        assert_eq!(
            v("2026-09-13 10:11:12.5"),
            Value::Timestamp("2026-09-13T10:11:12.5Z".into())
        );
        assert_eq!(
            v("2026-09-13t10:11:12+02:00"),
            Value::Timestamp("2026-09-13T10:11:12+02:00".into())
        );
        assert_eq!(v("2026-13-01"), Value::Str("2026-13-01".into()));
        assert_eq!(v("2026-09-13x"), Value::Str("2026-09-13x".into()));
    }

    #[test]
    fn quoted_scalars_are_strings_and_merge_keys_apply() {
        let doc = load(
            "a: \"12\"\nb: '~'\nc: 12\nbase: &b {x: 1, y: 2}\nd:\n  <<: *b\n  x: 3\ne: !reset null\nf: !!str 12\n",
            0,
        )
        .unwrap();
        let m = doc.as_map().unwrap();
        assert_eq!(m["a"].value, Value::Str("12".into()));
        assert_eq!(m["b"].value, Value::Str("~".into()));
        assert_eq!(m["c"].value, Value::Int(12));
        let d = m["d"].as_map().unwrap();
        assert_eq!(d["x"].value, Value::Int(3));
        assert_eq!(d["y"].value, Value::Int(2));
        assert_eq!(m["e"].tag.as_deref(), Some("!reset"));
        assert_eq!(m["e"].value, Value::Null);
        assert_eq!(m["f"].value, Value::Str("12".into()));
        assert_eq!(m["c"].span.line, 3);
    }
}
