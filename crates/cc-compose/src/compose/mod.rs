//! Lecture et normalisation d'un projet Compose.
//!
//! La chaîne suit celle de `compose-go` (`parsing.md`) : parse YAML en arbre
//! brut avec positions, clés en chaînes, interpolation, validation par le
//! schéma JSON, `extends`, `include`, canonicalisation, fusion des overrides.
//! Tout se fait sur l'arbre brut ; le typage vient en dernier.

pub mod merge;
pub mod node;
pub mod normalize;
pub mod project;
pub mod transform;
pub mod typed;
pub mod yaml;
