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
    /// Flavor imposée (sinon la plus petite qui couvre `deploy.resources`).
    /// Raccourci pour `min_flavor` = `max_flavor`.
    pub flavor: Option<String>,
    /// Scaling vertical : la plateforme change la taille des instances
    /// entre ces deux flavors selon la charge.
    pub min_flavor: Option<String>,
    pub max_flavor: Option<String>,
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
    /// Service `mongo` : `documentdb` (PostgreSQL dédié avec l'option, API
    /// MongoDB 7.0 ; défaut pour un tag 5+ ou absent) ou `mongodb` (addon
    /// MongoDB 4.0.3 ; défaut pour un tag 4.x).
    pub engine: Option<String>,
    /// Image custom : dialecte de `data/addon-images.json` (`postgres`,
    /// `mysql`, `redis`...) selon lequel ce service se configure ; il est
    /// alors substitué par l'addon correspondant, `build:` compris.
    pub dialect: Option<String>,
    /// Variables imposées sur Clever Cloud, qui l'emportent sur toute
    /// règle (corrélation, réécriture de nom). Valeur littérale, avec des
    /// références `{{service.rôle}}` (rôles `host`, `port`, `user`,
    /// `password`, `database`, `uri` d'un service substitué ; `host` d'un
    /// service quelconque du même network group).
    #[serde(default)]
    pub environment: indexmap::IndexMap<String, String>,
    /// Service substitué par un addon : où s'exécutent ses scripts
    /// d'initialisation. `task` (défaut : une Clever Task) ou `local`
    /// (depuis le poste qui lance `up`).
    pub init: Option<String>,
    /// Cellar (S3 managé), pris en compte sur Clever Cloud seulement.
    /// Niveau projet : les ressources, par nom logique (`buckets` créés
    /// s'ils manquent, `name` pour réutiliser un addon existant). Niveau
    /// service : les noms logiques à lier (variables `CELLAR_ADDON_*`,
    /// `CELLAR_ADDON_BUCKET`, références `{{n.host|key_id|key_secret|bucket}}`).
    #[serde(default)]
    pub cellar: Option<CellarField>,
    /// Niveau projet : conserver les fichiers du projet et le plan dans un
    /// bucket Cellar de l'organisation (relecture depuis la plateforme).
    pub assets: Option<bool>,
    /// Niveau projet : comment les réseaux Compose deviennent des network
    /// groups. `per-network` (un groupe par réseau), `single` (un seul
    /// groupe pour tout le projet), `none` (aucun : les services ne se
    /// joignent pas entre eux). Exigé dès que le projet a plus d'un réseau.
    pub network_groups: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum CellarField {
    /// Niveau service : `cellar: [files]`.
    Refs(Vec<String>),
    /// Niveau projet : `cellar: { files: { buckets: [...] } }`.
    Decls(indexmap::IndexMap<String, CellarDecl>),
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CellarDecl {
    #[serde(default)]
    pub buckets: Vec<String>,
    /// Nom réel d'un addon Cellar de l'organisation à réutiliser ; sinon
    /// `<projet>-<nom logique>`, créé au besoin.
    #[serde(default)]
    pub name: Option<String>,
}

impl XClever {
    /// Les Cellar déclarés (niveau projet).
    pub fn cellar_decls(&self) -> indexmap::IndexMap<String, CellarDecl> {
        match &self.cellar {
            Some(CellarField::Decls(d)) => d.clone(),
            _ => Default::default(),
        }
    }

    /// Les Cellar liés (niveau service).
    pub fn cellar_refs(&self) -> Vec<String> {
        match &self.cellar {
            Some(CellarField::Refs(r)) => r.clone(),
            _ => Vec::new(),
        }
    }

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
            if let Some(ng) = &x.network_groups {
                if !["per-network", "single", "none"].contains(&ng.as_str()) {
                    diags.push(Diagnostic::error("x-clever-invalid", format!("`network_groups` vaut `per-network`, `single` ou `none`, pas `{ng}`")).with_path(&p.child("network_groups")));
                    x.network_groups = None;
                }
            }
            if let Some(e) = &x.engine {
                if e != "documentdb" && e != "mongodb" {
                    diags.push(Diagnostic::error("x-clever-invalid", format!("`engine` vaut `documentdb` ou `mongodb`, pas `{e}`")).with_path(&p.child("engine")));
                    x.engine = None;
                }
            }
            if let Some(d) = &x.dialect {
                if crate::plan::addons::dialect(d).is_none() {
                    let known: Vec<&str> = crate::plan::addons::images().dialects.keys().map(String::as_str).collect();
                    diags.push(Diagnostic::error("x-clever-invalid", format!("`dialect` inconnu `{d}` ; dialectes : {}", known.join(", "))).with_path(&p.child("dialect")));
                    x.dialect = None;
                }
            }
            if let Some(init) = &x.init {
                if init != "task" && init != "local" {
                    diags.push(Diagnostic::error("x-clever-invalid", format!("`init` vaut `task` ou `local`, pas `{init}`")).with_path(&p.child("init")));
                    x.init = None;
                }
            }
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
