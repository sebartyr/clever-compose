//! Validation par le schéma JSON de la spécification Compose, embarqué
//! depuis compose-go (voir `schema/PROVENANCE.md`). C'est ce qui rejette une
//! clé inconnue ou un type invalide, avec le chemin fautif ; compose-go
//! l'applique après la fusion de chaque fichier.

use std::sync::OnceLock;

use jsonschema::Validator;

use super::node::{Node, Path};
use crate::diagnostic::Diagnostic;

const SCHEMA: &str = include_str!("../../schema/compose-spec.json");

fn validator() -> &'static Validator {
    static V: OnceLock<Validator> = OnceLock::new();
    V.get_or_init(|| {
        let schema: serde_json::Value = serde_json::from_str(SCHEMA).expect("schéma compose-spec.json invalide");
        jsonschema::options().build(&schema).expect("schéma compose-spec.json non compilable")
    })
}

/// Valide l'arbre ; renvoie l'erreur la plus spécifique, avec le chemin et
/// la position du nœud fautif.
pub fn validate(tree: &Node) -> Result<(), Diagnostic> {
    let json = tree.to_json();
    let mut errors: Vec<jsonschema::ValidationError<'_>> = validator().iter_errors(&json).collect();
    if errors.is_empty() {
        return Ok(());
    }
    // La plus spécifique : le chemin d'instance le plus long, comme
    // `getMostSpecificError` de compose-go.
    errors.sort_by_key(|e| std::cmp::Reverse(e.instance_path().iter().count()));
    let e = errors.remove(0);
    let path = Path::from_segments(e.instance_path().iter().map(|seg| seg.to_string()).collect());
    let span = find_span(tree, path.segments());
    let message = format!("{path} {}", e);
    let mut d = Diagnostic::error("schema", message).with_path(&path);
    if let Some(s) = span {
        d.span = Some(s);
    }
    Err(d)
}

pub fn find_span(node: &Node, segments: &[String]) -> Option<super::node::Span> {
    let mut cur = node;
    for seg in segments {
        cur = match &cur.value {
            super::node::Value::Map(m) => m.get(seg)?,
            super::node::Value::Seq(s) => s.get(seg.trim_matches(['[', ']']).parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur.span)
}
