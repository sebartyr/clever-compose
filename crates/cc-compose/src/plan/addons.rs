//! Substitution des services stateful par des addons managés (CLAUDE.md §6).
//!
//! Reconnaissance par image (fournisseur cible **et** dialecte de variables
//! du service source), version depuis le tag, plan le moins cher de la
//! région sauf `x-clever.plan`, coût annoncé. Câblage des identifiants des
//! consommateurs : URL reconstruite, variables discrètes corrélées par
//! égalité de valeur, formats non standards ; tout passe par des références
//! `AddonEnv` expansées par le shim au démarrage (`CC_COMPOSE_EXPAND`),
//! jamais figées. Falaises sémantiques diagnostiquées (§6.4).

use std::sync::OnceLock;

use cc_compose_model::plan::{Reference, ResourceId, Segment, Template};
use serde::Deserialize;

use crate::compose::model::Service;

#[derive(Debug, Deserialize)]
pub struct Catalog {
    pub providers: indexmap::IndexMap<String, Provider>,
}

#[derive(Debug, Deserialize)]
pub struct Provider {
    pub name: String,
    pub plans: Vec<PlanInfo>,
    #[serde(default)]
    pub versions: Vec<String>,
    #[serde(default)]
    pub default_version: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PlanInfo {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub price: f64,
    #[serde(default)]
    pub zones: Vec<String>,
    #[serde(default)]
    pub features: indexmap::IndexMap<String, String>,
}

pub fn catalog() -> &'static Catalog {
    static C: OnceLock<Catalog> = OnceLock::new();
    C.get_or_init(|| serde_json::from_str(include_str!("../../data/addon-providers.json")).expect("data/addon-providers.json invalide"))
}

/// Dialecte des variables du service source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// Images officielles Docker Hub.
    Official,
    /// Images Bitnami (`POSTGRESQL_*`, `MARIADB_*`...).
    Bitnami,
}

#[derive(Debug, Clone)]
pub struct Recognized {
    pub provider: &'static str,
    pub dialect: Dialect,
    /// Nom court pour les messages (`postgres`, `mariadb`...).
    pub kind: &'static str,
    /// Version demandée par le tag (`16`, `8.0`), si lisible.
    pub version: Option<String>,
    /// Port par défaut du service conteneurisé.
    pub port: u16,
}

/// Reconnaissance d'une image. Jamais un service `build:` (§6.1).
pub fn recognize(image: &str) -> Option<Recognized> {
    let (name, tag) = image.rsplit_once('@').map(|(n, _)| (n, None)).unwrap_or_else(|| match image.rsplit_once(':') {
        Some((n, t)) if !t.contains('/') => (n, Some(t)),
        _ => (image, None),
    });
    let name = name.trim_start_matches("docker.io/").trim_start_matches("library/");
    let (provider, dialect, kind, port, major_only) = match name {
        "postgres" | "postgis/postgis" | "pgvector/pgvector" | "timescale/timescaledb" | "timescale/timescaledb-ha" => ("postgresql-addon", Dialect::Official, "postgres", 5432, true),
        "bitnami/postgresql" => ("postgresql-addon", Dialect::Bitnami, "postgres", 5432, true),
        "mysql" => ("mysql-addon", Dialect::Official, "mysql", 3306, false),
        "bitnami/mysql" => ("mysql-addon", Dialect::Bitnami, "mysql", 3306, false),
        "mariadb" => ("mysql-addon", Dialect::Official, "mariadb", 3306, false),
        "bitnami/mariadb" => ("mysql-addon", Dialect::Bitnami, "mariadb", 3306, false),
        "redis" | "valkey/valkey" | "redis/redis-stack-server" => ("redis-addon", Dialect::Official, "redis", 6379, true),
        "bitnami/redis" | "bitnami/valkey" => ("redis-addon", Dialect::Bitnami, "redis", 6379, true),
        "mongo" => ("mongodb-addon", Dialect::Official, "mongo", 27017, false),
        "bitnami/mongodb" => ("mongodb-addon", Dialect::Bitnami, "mongo", 27017, false),
        "elasticsearch" | "docker.elastic.co/elasticsearch/elasticsearch" => ("es-addon", Dialect::Official, "elasticsearch", 9200, true),
        "bitnami/elasticsearch" => ("es-addon", Dialect::Bitnami, "elasticsearch", 9200, true),
        _ => return None,
    };
    let version = tag.filter(|t| *t != "latest").and_then(|t| {
        let digits: String = t.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        if digits.is_empty() {
            return None;
        }
        let parts: Vec<&str> = digits.trim_end_matches('.').split('.').collect();
        Some(if major_only { parts[0].to_string() } else { parts.iter().take(2).cloned().collect::<Vec<_>>().join(".") })
    });
    Some(Recognized { provider, dialect, kind, version, port })
}

/// Identifiants déclarés par le service source, selon son dialecte.
#[derive(Debug, Clone, Default)]
pub struct Declared {
    pub user: Option<String>,
    pub password: Option<String>,
    pub database: Option<String>,
    pub root_password: Option<String>,
}

pub fn declared(r: &Recognized, s: &Service) -> Declared {
    let get = |k: &str| s.environment.get(k).and_then(|v| v.clone()).filter(|v| !v.is_empty());
    let mut d = Declared::default();
    match (r.provider, r.dialect) {
        ("postgresql-addon", Dialect::Official) => {
            d.user = get("POSTGRES_USER").or_else(|| Some("postgres".into()));
            d.password = get("POSTGRES_PASSWORD");
            d.database = get("POSTGRES_DB").or_else(|| d.user.clone());
        }
        ("postgresql-addon", Dialect::Bitnami) => {
            d.user = get("POSTGRESQL_USERNAME").or_else(|| Some("postgres".into()));
            d.password = get("POSTGRESQL_PASSWORD").or_else(|| get("POSTGRES_PASSWORD"));
            d.database = get("POSTGRESQL_DATABASE").or_else(|| Some("postgres".into()));
        }
        ("mysql-addon", _) => {
            let prefix = if r.kind == "mariadb" && r.dialect == Dialect::Bitnami { "MARIADB_" } else { "MYSQL_" };
            d.root_password = get(&format!("{prefix}ROOT_PASSWORD")).or_else(|| get("MYSQL_ROOT_PASSWORD")).or_else(|| get("MARIADB_ROOT_PASSWORD"));
            d.user = get(&format!("{prefix}USER")).or_else(|| get("MYSQL_USER")).or_else(|| get("MARIADB_USER"));
            d.password = get(&format!("{prefix}PASSWORD")).or_else(|| get("MYSQL_PASSWORD")).or_else(|| get("MARIADB_PASSWORD"));
            d.database = get(&format!("{prefix}DATABASE")).or_else(|| get("MYSQL_DATABASE")).or_else(|| get("MARIADB_DATABASE"));
        }
        ("redis-addon", Dialect::Official) => {
            // `--requirepass X` dans la commande, sinon sans mot de passe.
            let cmd = s.command.clone().unwrap_or_default();
            d.password = cmd.iter().position(|a| a == "--requirepass").and_then(|i| cmd.get(i + 1).cloned());
        }
        ("redis-addon", Dialect::Bitnami) => d.password = get("REDIS_PASSWORD"),
        ("mongodb-addon", Dialect::Official) => {
            d.user = get("MONGO_INITDB_ROOT_USERNAME");
            d.password = get("MONGO_INITDB_ROOT_PASSWORD");
            d.database = get("MONGO_INITDB_DATABASE");
        }
        ("mongodb-addon", Dialect::Bitnami) => {
            d.user = get("MONGODB_ROOT_USER").or_else(|| Some("root".into()));
            d.password = get("MONGODB_ROOT_PASSWORD");
            d.database = get("MONGODB_DATABASE");
        }
        ("es-addon", _) => {
            d.user = Some("elastic".into());
            d.password = get("ELASTIC_PASSWORD");
        }
        _ => {}
    }
    d
}

/// Variables injectées par la liaison de l'addon, par rôle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Host,
    Port,
    User,
    Password,
    Database,
    Uri,
}

pub fn addon_var(provider: &str, role: Role) -> Option<&'static str> {
    Some(match (provider, role) {
        ("postgresql-addon", Role::Host) => "POSTGRESQL_ADDON_HOST",
        ("postgresql-addon", Role::Port) => "POSTGRESQL_ADDON_PORT",
        ("postgresql-addon", Role::User) => "POSTGRESQL_ADDON_USER",
        ("postgresql-addon", Role::Password) => "POSTGRESQL_ADDON_PASSWORD",
        ("postgresql-addon", Role::Database) => "POSTGRESQL_ADDON_DB",
        ("postgresql-addon", Role::Uri) => "POSTGRESQL_ADDON_URI",
        ("mysql-addon", Role::Host) => "MYSQL_ADDON_HOST",
        ("mysql-addon", Role::Port) => "MYSQL_ADDON_PORT",
        ("mysql-addon", Role::User) => "MYSQL_ADDON_USER",
        ("mysql-addon", Role::Password) => "MYSQL_ADDON_PASSWORD",
        ("mysql-addon", Role::Database) => "MYSQL_ADDON_DB",
        ("mysql-addon", Role::Uri) => "MYSQL_ADDON_URI",
        ("redis-addon", Role::Host) => "REDIS_HOST",
        ("redis-addon", Role::Port) => "REDIS_PORT",
        ("redis-addon", Role::Password) => "REDIS_PASSWORD",
        ("redis-addon", Role::Uri) => "REDIS_URL",
        ("mongodb-addon", Role::Host) => "MONGODB_ADDON_HOST",
        ("mongodb-addon", Role::Port) => "MONGODB_ADDON_PORT",
        ("mongodb-addon", Role::User) => "MONGODB_ADDON_USER",
        ("mongodb-addon", Role::Password) => "MONGODB_ADDON_PASSWORD",
        ("mongodb-addon", Role::Database) => "MONGODB_ADDON_DB",
        ("mongodb-addon", Role::Uri) => "MONGODB_ADDON_URI",
        ("es-addon", Role::Host) => "ES_ADDON_HOST",
        ("es-addon", Role::User) => "ES_ADDON_USER",
        ("es-addon", Role::Password) => "ES_ADDON_PASSWORD",
        ("es-addon", Role::Uri) => "ES_ADDON_URI",
        _ => return None,
    })
}

pub fn seg(addon: &str, provider: &str, role: Role) -> Option<Segment> {
    addon_var(provider, role).map(|var| Segment::Ref(Reference::AddonEnv { addon: ResourceId(format!("addon/{addon}")), var: var.to_string() }))
}

/// URL complète reconstruite depuis les attributs de l'addon, au format
/// attendu par l'application : schéma, chemin et query d'origine conservés
/// (§6.2, cas 1 et 3).
pub fn rebuild_url(original: &str, addon: &str, provider: &str) -> Option<Template> {
    let (scheme, rest) = original.split_once("://")?;
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (_authority, tail) = rest.split_at(authority_end);
    let mut segs: Vec<Segment> = vec![Segment::Literal(format!("{scheme}://"))];
    let lit = |segs: &mut Vec<Segment>, s: &str| segs.push(Segment::Literal(s.to_string()));
    let push = |segs: &mut Vec<Segment>, s: Option<Segment>| {
        if let Some(s) = s {
            segs.push(s);
        }
    };
    match provider {
        "redis-addon" => {
            // `redis://default:<password>@<host>:<port>` (doc Clever pour Docs).
            lit(&mut segs, "default:");
            push(&mut segs, seg(addon, provider, Role::Password));
            lit(&mut segs, "@");
            push(&mut segs, seg(addon, provider, Role::Host));
            lit(&mut segs, ":");
            push(&mut segs, seg(addon, provider, Role::Port));
            lit(&mut segs, tail);
        }
        "es-addon" => {
            // L'addon ne sert que HTTPS sur 443 (`ES_ADDON_HOST` est un hôte
            // nu, constaté le 13/09/2026) : le schéma d'origine ne tient pas.
            segs[0] = Segment::Literal("https://".into());
            push(&mut segs, seg(addon, provider, Role::User));
            lit(&mut segs, ":");
            push(&mut segs, seg(addon, provider, Role::Password));
            lit(&mut segs, "@");
            push(&mut segs, seg(addon, provider, Role::Host));
            lit(&mut segs, tail);
        }
        _ => {
            push(&mut segs, seg(addon, provider, Role::User));
            lit(&mut segs, ":");
            push(&mut segs, seg(addon, provider, Role::Password));
            lit(&mut segs, "@");
            push(&mut segs, seg(addon, provider, Role::Host));
            lit(&mut segs, ":");
            push(&mut segs, seg(addon, provider, Role::Port));
            // Le chemin est la base : celle de l'addon.
            let query = tail.find(['?', '#']).map(|i| &tail[i..]).unwrap_or("");
            lit(&mut segs, "/");
            push(&mut segs, seg(addon, provider, Role::Database));
            if !query.is_empty() {
                lit(&mut segs, query);
            }
        }
    }
    Some(Template(segs))
}

/// Plan retenu : `wanted` (slug) s'il existe, sinon le moins cher servi
/// dans la région.
pub fn choose_plan<'a>(provider: &'a Provider, region: &str, wanted: Option<&str>) -> Result<&'a PlanInfo, String> {
    if let Some(w) = wanted {
        return provider.plans.iter().find(|p| p.slug == w || p.name.eq_ignore_ascii_case(w)).ok_or_else(|| format!("plan `{w}` inconnu ; disponibles : {}", provider.plans.iter().map(|p| p.slug.as_str()).collect::<Vec<_>>().join(", ")));
    }
    provider.plans.iter().filter(|p| p.zones.is_empty() || p.zones.iter().any(|z| z == region)).min_by(|a, b| a.price.partial_cmp(&b.price).unwrap()).ok_or_else(|| format!("aucun plan de {} dans la région {region}", provider.name))
}

/// Limite de connexions d'un plan, si le catalogue la donne.
pub fn max_connections(plan: &PlanInfo) -> Option<u32> {
    plan.features.get("Max connection limit").and_then(|v| v.trim().parse().ok())
}
