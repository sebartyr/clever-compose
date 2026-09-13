//! L'exécuteur : applique le plan sur la plateforme (CLAUDE.md §7, phase 3).
//! Sans état local : tout se reconstruit depuis l'API par les noms et les
//! tags. Ordre : groupes, applications sans déploiement, liaisons,
//! variables (FQDN calculés), redirections, domaines, puis push et
//! déploiement dans l'ordre topologique, en attendant chaque succès.

pub mod git;
pub mod inspect;
pub mod local;
pub mod platform;
pub mod reconcile;
