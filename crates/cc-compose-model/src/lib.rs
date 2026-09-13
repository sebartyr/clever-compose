//! Types partagés entre la CLI et le shim.
//!
//! Ce crate est lu par le shim (binaire statique musl), donc ses dépendances
//! restent frugales. Le plan intermédiaire viendra ici en phase 1 ; si l'arbre
//! grossit, scinder en `model-core` (ce que le shim lit) et `model-plan`.

pub mod plan;
pub mod shim;
