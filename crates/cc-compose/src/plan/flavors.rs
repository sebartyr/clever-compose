//! Flavors Docker (`data/docker-instance.json`, voir `data/PROVENANCE.md`)
//! et traduction de `deploy` en scaling.

use std::sync::OnceLock;

use cc_compose_model::plan::Scaling;
use serde::Deserialize;

use super::xclever::XClever;
use crate::compose::model::Service;
use crate::compose::node::Path;
use crate::compose::typed::parse_ram_in_bytes;
use crate::diagnostic::{Diagnostic, Diagnostics};

#[derive(Debug, Deserialize)]
pub struct Instance {
    #[serde(rename = "defaultFlavor")]
    pub default_flavor: String,
    #[serde(rename = "maxInstances")]
    pub max_instances: u32,
    pub flavors: Vec<Flavor>,
}

#[derive(Debug, Deserialize)]
pub struct Flavor {
    pub name: String,
    #[serde(rename = "memoryBytes")]
    pub memory_bytes: u64,
    pub cpus: u32,
}

pub fn docker() -> &'static Instance {
    static I: OnceLock<Instance> = OnceLock::new();
    I.get_or_init(|| serde_json::from_str(include_str!("../../data/docker-instance.json")).expect("data/docker-instance.json invalide"))
}

/// La plus petite flavor dont la mémoire couvre la demande.
pub fn nearest_by_memory(bytes: u64) -> &'static Flavor {
    let d = docker();
    d.flavors.iter().find(|f| f.memory_bytes >= bytes).unwrap_or_else(|| d.flavors.last().expect("table des flavors vide"))
}

pub fn scaling(s: &Service, x: &XClever, px: &XClever, p: &Path, diags: &mut Diagnostics) -> Scaling {
    let d = docker();
    let requested = s
        .deploy
        .as_ref()
        .and_then(|dp| dp.resources.as_ref())
        .and_then(|r| r.limits.as_ref().or(r.reservations.as_ref()))
        .and_then(|l| l.memory.clone())
        .or_else(|| s.mem_limit.clone());
    let flavor = match x.flavor.clone().or_else(|| px.flavor.clone()) {
        Some(f) => {
            if !d.flavors.iter().any(|fl| fl.name == f) {
                diags.push(Diagnostic::error("flavor-unknown", format!("flavor `{f}` inconnue ; disponibles : {}", d.flavors.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", "))).with_path(&p.child("x-clever").child("flavor")));
            }
            f
        }
        None => match requested {
            Some(mem) => match parse_ram_in_bytes(&mem) {
                Some(bytes) => {
                    let f = nearest_by_memory(bytes as u64);
                    if (f.memory_bytes as i64) < bytes {
                        diags.push(Diagnostic::warning("memory-capped", format!("mémoire demandée {mem} au-delà de la plus grande flavor ({}) ; plafonnée", f.name)).with_path(p));
                    }
                    f.name.clone()
                }
                None => {
                    diags.push(Diagnostic::error("memory-invalid", format!("mémoire invalide `{mem}`")).with_path(p));
                    d.default_flavor.clone()
                }
            },
            None => d.default_flavor.clone(),
        },
    };
    let replicas = s.deploy.as_ref().and_then(|dp| dp.replicas).or(s.scale).unwrap_or(1).max(1) as u32;
    let min = x.min_instances.unwrap_or(replicas);
    let max = x.max_instances.unwrap_or(min.max(replicas));
    if max > d.max_instances {
        diags.push(Diagnostic::error("instances-too-many", format!("{max} instances demandées, maximum {}", d.max_instances)).with_path(p));
    }
    Scaling { flavor, min_instances: min, max_instances: max }
}
