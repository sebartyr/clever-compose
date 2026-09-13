//! Arbre brut d'un document Compose, avec positions.
//!
//! Chaque nœud garde la position de son origine pour les diagnostics. Les clés
//! de mapping sont toujours des chaînes (compose-go, phase 2) et l'ordre des
//! clés est conservé.

use std::fmt;

use indexmap::IndexMap;

/// Position d'un nœud dans son fichier source. Lignes et colonnes à partir
/// de 1 ; `file` est un index dans la table des fichiers du projet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub file: usize,
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub span: Span,
    pub value: Value,
    /// Tag applicatif porté par le nœud (`!reset`, `!override`), consommé par
    /// la fusion. Les tags du schéma core YAML sont résolus au chargement et
    /// n'apparaissent pas ici.
    pub tag: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    /// Chaîne, quotée ou non. La résolution a déjà eu lieu : un scalaire
    /// non quoté qui n'a pas été reconnu comme un autre type arrive ici.
    Str(String),
    /// Horodatage reconnu sur un scalaire non quoté, gardé sous sa forme
    /// textuelle d'origine (compose-go le ressort en RFC 3339).
    Timestamp(String),
    Seq(Vec<Node>),
    Map(IndexMap<String, Node>),
}

impl Node {
    pub fn new(span: Span, value: Value) -> Self {
        Node { span, value, tag: None }
    }

    pub fn as_map(&self) -> Option<&IndexMap<String, Node>> {
        match &self.value {
            Value::Map(m) => Some(m),
            _ => None,
        }
    }

    pub fn as_map_mut(&mut self) -> Option<&mut IndexMap<String, Node>> {
        match &mut self.value {
            Value::Map(m) => Some(m),
            _ => None,
        }
    }

    pub fn as_seq(&self) -> Option<&[Node]> {
        match &self.value {
            Value::Seq(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match &self.value {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self.value, Value::Null)
    }

    /// Conversion en JSON, pour la validation par schéma et la comparaison
    /// avec l'oracle. Les positions sont perdues.
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::Value as J;
        match &self.value {
            Value::Null => J::Null,
            Value::Bool(b) => J::Bool(*b),
            Value::Int(i) => J::from(*i),
            Value::Float(f) => serde_json::Number::from_f64(*f).map(J::Number).unwrap_or(J::Null),
            Value::Str(s) | Value::Timestamp(s) => J::String(s.clone()),
            Value::Seq(s) => J::Array(s.iter().map(Node::to_json).collect()),
            Value::Map(m) => J::Object(m.iter().map(|(k, v)| (k.clone(), v.to_json())).collect()),
        }
    }
}

/// Chemin d'un nœud dans l'arbre, à la manière de `tree.Path` de compose-go :
/// `services.web.ports[0]`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Path(Vec<String>);

impl Path {
    pub fn root() -> Self {
        Path(Vec::new())
    }

    pub fn child(&self, key: &str) -> Self {
        let mut p = self.0.clone();
        p.push(key.to_string());
        Path(p)
    }

    pub fn index(&self, i: usize) -> Self {
        let mut p = self.0.clone();
        p.push(format!("[{i}]"));
        Path(p)
    }

    pub fn segments(&self) -> &[String] {
        &self.0
    }

    /// Correspondance avec un motif où `*` remplace un segment et `[]` un
    /// index de liste, comme `tree.Path.Matches` de compose-go
    /// (`services.*.command`, `services.*.ports.[].target`). `*` accepte
    /// aussi un index.
    pub fn matches(&self, pattern: &str) -> bool {
        let pat: Vec<&str> = pattern.split('.').collect();
        pat.len() == self.0.len()
            && pat.iter().zip(&self.0).all(|(p, s)| *p == "*" || (*p == "[]" && s.starts_with('[')) || *p == s.as_str())
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut first = true;
        for seg in &self.0 {
            if !seg.starts_with('[') && !first {
                f.write_str(".")?;
            }
            f.write_str(seg)?;
            first = false;
        }
        Ok(())
    }
}
