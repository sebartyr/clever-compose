//! Lecture et normalisation d'un projet Compose.
//!
//! La chaîne suit celle de `compose-go` (`parsing.md`) : parse YAML en arbre
//! brut avec positions, clés en chaînes, interpolation, validation par le
//! schéma JSON, `extends`, `include`, canonicalisation, fusion des overrides.
//! Tout se fait sur l'arbre brut ; le typage vient en dernier.

pub mod consistency;
pub mod dotenv;
pub mod edit;
pub mod envfile;
pub mod interpolate;
pub mod loader;
pub mod merge;
pub mod model;
pub mod node;
pub mod normalize;
pub mod paths;
pub mod project;
pub mod schema;
pub mod template;
pub mod transform;
pub mod typed;
pub mod yaml;
