//! Extension `x-clever`, au niveau projet ou service (CLAUDE.md §6).
//! Les clés inconnues produisent un diagnostic.

use serde::Deserialize;

use crate::compose::model::{Project, Service};
use crate::compose::node::Path;
use crate::diagnostic::{Diagnostic, Diagnostics};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct XClever {
    /// Région plateforme (`par` par défaut).
    pub region: Option<String>,
    /// Flavor imposée (sinon la plus proche en mémoire des limites).
    pub flavor: Option<String>,
    pub min_instances: Option<u32>,
    pub max_instances: Option<u32>,
    /// Service one-shot à déployer en Clever Task.
    pub task: Option<bool>,
    /// Port HTTP réel de l'image, relayé depuis 8080.
    pub http_port: Option<u16>,
    /// `http` ou `tcp` : nature du port principal.
    pub protocol: Option<String>,
    /// Domaines publics.
    #[serde(default, alias = "domain")]
    pub domains: Vec<String>,
    /// Accepte des volumes éphémères pour ce service.
    pub ephemeral: Option<bool>,
    /// Plan d'addon en cas de substitution.
    pub plan: Option<String>,
    /// Rattacher l'addon au network group.
    pub private: Option<bool>,
    /// Namespace des redirections TCP/UDP (`default` par défaut).
    pub tcp_namespace: Option<String>,
    /// Version de l'addon (`16`, `8.0`) si le tag ne la donne pas ou pour
    /// la forcer.
    pub version: Option<String>,
}

impl XClever {
    pub fn project(project: &Project, diags: &mut Diagnostics) -> XClever {
        match project.extra.get("x-clever") {
            Some(v) => parse(v, &Path::root().child("x-clever"), diags),
            None => XClever::default(),
        }
    }

    pub fn service(s: &Service, p: &Path, diags: &mut Diagnostics) -> XClever {
        match s.extra.get("x-clever") {
            Some(v) => parse(v, &p.child("x-clever"), diags),
            None => XClever::default(),
        }
    }
}

fn parse(v: &serde_json::Value, p: &Path, diags: &mut Diagnostics) -> XClever {
    match serde_json::from_value::<XClever>(v.clone()) {
        Ok(mut x) => {
            if let Some(proto) = &x.protocol {
                if proto != "http" && proto != "tcp" {
                    diags.push(Diagnostic::error("x-clever-invalid", format!("`protocol` vaut `http` ou `tcp`, pas `{proto}`")).with_path(&p.child("protocol")));
                    x.protocol = None;
                }
            }
            x
        }
        Err(e) => {
            diags.push(Diagnostic::error("x-clever-invalid", format!("extension x-clever invalide : {e}")).with_path(p));
            XClever::default()
        }
    }
}
