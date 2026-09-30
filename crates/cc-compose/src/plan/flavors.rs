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

/// La plus petite flavor qui couvre la mémoire et les vCPU demandés (l'un
/// ou l'autre peut être absent : 0).
pub fn nearest(memory_bytes: u64, cpus: f64) -> &'static Flavor {
    let d = docker();
    d.flavors.iter().find(|f| f.memory_bytes >= memory_bytes && f.cpus as f64 >= cpus).unwrap_or_else(|| d.flavors.last().expect("table des flavors vide"))
}

/// `deploy.resources.limits` (sinon `reservations`, sinon `mem_limit` /
/// `cpus` de premier niveau) en octets et vCPU demandés.
fn requested(s: &Service, p: &Path, diags: &mut Diagnostics) -> (Option<u64>, Option<f64>) {
    let spec = s.deploy.as_ref().and_then(|dp| dp.resources.as_ref()).and_then(|r| r.limits.as_ref().or(r.reservations.as_ref()));
    let mem_text = spec.and_then(|l| l.memory.clone()).or_else(|| s.mem_limit.clone());
    let mem = match &mem_text {
        Some(m) => match parse_ram_in_bytes(m) {
            Some(b) => Some(b as u64),
            None => {
                diags.push(Diagnostic::error("memory-invalid", format!("mémoire invalide `{m}`")).with_path(p));
                None
            }
        },
        None => None,
    };
    let cpus = spec.and_then(|l| l.cpus.as_ref()).and_then(|c| c.as_f64().or_else(|| c.as_str().and_then(|t| t.parse().ok()))).or(s.cpus);
    (mem, cpus)
}

fn known(f: &str, p: &Path, diags: &mut Diagnostics) -> bool {
    let d = docker();
    if d.flavors.iter().any(|fl| fl.name == f) {
        return true;
    }
    diags.push(Diagnostic::error("flavor-unknown", format!("flavor `{f}` inconnue ; disponibles : {}", d.flavors.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", "))).with_path(p));
    false
}

fn rank(f: &str) -> usize {
    docker().flavors.iter().position(|fl| fl.name == f).unwrap_or(0)
}

/// Traduction de `deploy` et de `x-clever` en scaling : la flavor vient de
/// `x-clever.flavor` (ou `min_flavor`/`max_flavor` pour le vertical), sinon
/// de `deploy.resources` (mémoire et vCPU) ; les instances de
/// `x-clever.min_instances`/`max_instances`, sinon de `deploy.replicas`.
pub fn scaling(s: &Service, x: &XClever, px: &XClever, default_build: Option<&str>, p: &Path, diags: &mut Diagnostics) -> Scaling {
    let d = docker();
    let xp = p.child("x-clever");
    let (mem, cpus) = requested(s, p, diags);
    // Flavor dérivée des ressources ; l'avertissement de plafonnement n'a de
    // sens que si elle sert.
    let (derived, capped) = match (mem, cpus) {
        (None, None) => (d.default_flavor.clone(), false),
        _ => {
            let f = nearest(mem.unwrap_or(0), cpus.unwrap_or(0.0));
            (f.name.clone(), mem.is_some_and(|m| f.memory_bytes < m) || cpus.is_some_and(|c| (f.cpus as f64) < c))
        }
    };
    let warn_capped = |diags: &mut Diagnostics| {
        if capped {
            diags.push(Diagnostic::warning("resources-capped", format!("ressources demandées ({}{}) au-delà de la plus grande flavor ({derived}) ; plafonnées", mem.map(|m| format!("{} Mio", m >> 20)).unwrap_or_default(), cpus.map(|c| format!(", {c} vCPU")).unwrap_or_default())).with_path(p));
        }
    };
    let pick = |own: &Option<String>, parent: &Option<String>| own.clone().or_else(|| parent.clone());
    let (min_flavor, max_flavor) = match (pick(&x.flavor, &px.flavor), pick(&x.min_flavor, &px.min_flavor), pick(&x.max_flavor, &px.max_flavor)) {
        (Some(f), None, None) => {
            known(&f, &xp.child("flavor"), diags);
            (f.clone(), f)
        }
        (Some(_), _, _) => {
            diags.push(Diagnostic::error("x-clever-invalid", "`flavor` et `min_flavor`/`max_flavor` s'excluent".to_string()).with_path(&xp.child("flavor")));
            (d.default_flavor.clone(), d.default_flavor.clone())
        }
        (None, lo, hi) if lo.is_some() || hi.is_some() => {
            let lo = lo.unwrap_or_else(|| {
                warn_capped(diags);
                derived.clone()
            });
            let hi = hi.unwrap_or_else(|| lo.clone());
            let ok = known(&lo, &xp.child("min_flavor"), diags) && known(&hi, &xp.child("max_flavor"), diags);
            if ok && rank(&lo) > rank(&hi) {
                diags.push(Diagnostic::error("x-clever-invalid", format!("`min_flavor` {lo} est plus grande que `max_flavor` {hi}")).with_path(&xp.child("min_flavor")));
            }
            (lo, hi)
        }
        _ => {
            warn_capped(diags);
            (derived.clone(), derived.clone())
        }
    };
    let replicas = s.deploy.as_ref().and_then(|dp| dp.replicas).or(s.scale).map(|r| r.max(1) as u32);
    let (min, max) = match (x.min_instances.or(px.min_instances), x.max_instances.or(px.max_instances)) {
        (None, None) => {
            let r = replicas.unwrap_or(1);
            if let Some(r) = replicas.filter(|r| *r > 1) {
                diags.push(Diagnostic::warning("replicas-fixed", format!("`replicas: {r}` donne {r} instances fixes, sans autoscaling ; `x-clever.min_instances`/`max_instances` pour laisser la plateforme en ajouter et retirer selon la charge")).with_path(&p.child("deploy").child("replicas")));
            }
            (r, r)
        }
        (lo, hi) => {
            let lo = lo.unwrap_or_else(|| replicas.unwrap_or(1).min(hi.unwrap_or(u32::MAX)));
            let hi = hi.unwrap_or_else(|| lo.max(replicas.unwrap_or(1)));
            if lo > hi {
                diags.push(Diagnostic::error("x-clever-invalid", format!("`min_instances` {lo} dépasse `max_instances` {hi}")).with_path(&xp.child("min_instances")));
            }
            (lo, hi)
        }
    };
    if max > d.max_instances {
        diags.push(Diagnostic::error("instances-too-many", format!("{max} instances demandées, maximum {}", d.max_instances)).with_path(p));
    }
    let build_flavor = build_flavor(x, px, default_build, &xp, diags);
    Scaling { min_flavor, max_flavor, min_instances: min, max_instances: max, build_flavor }
}

/// Instance de build dédiée : `x-clever.build_flavor` du service, sinon du
/// projet, sinon `--build-flavor` ; `disabled` l'écarte explicitement.
/// Aucune par défaut (le build se fait alors sur l'instance d'exécution).
fn build_flavor(x: &XClever, px: &XClever, default_build: Option<&str>, xp: &Path, diags: &mut Diagnostics) -> Option<String> {
    let (value, path) = match (&x.build_flavor, &px.build_flavor) {
        (Some(v), _) => (v.clone(), Some(xp.child("build_flavor"))),
        (None, Some(v)) => (v.clone(), Some(Path::root().child("x-clever").child("build_flavor"))),
        (None, None) => (default_build?.to_string(), None),
    };
    if value == "disabled" {
        return None;
    }
    let d = docker();
    if d.flavors.iter().any(|f| f.name == value) {
        return Some(value);
    }
    let msg = format!("flavor de build `{value}` inconnue ; disponibles : {}, ou `disabled`", d.flavors.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", "));
    let diag = Diagnostic::error("flavor-unknown", msg);
    diags.push(match path {
        Some(p) => diag.with_path(&p),
        None => diag,
    });
    None
}
