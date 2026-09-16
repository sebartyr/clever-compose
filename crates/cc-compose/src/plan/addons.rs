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

use cc_compose_model::plan::{addon_ng_port, Reference, ResourceId, Segment, Template};
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
    /// Version servie par le cluster partagé de chaque zone (plans
    /// `Shared`, comme `dev`) : la version demandée n'y a aucun effet.
    #[serde(default)]
    pub shared_versions: indexmap::IndexMap<String, String>,
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

/// Moteur interne : un PostgreSQL dédié (17+) créé avec l'option
/// `documentdb`, qui expose un endpoint compatible MongoDB 7.0
/// (`DOCUMENTDB_ADDON_*`, identifiants de `POSTGRESQL_ADDON_*`, TLS
/// obligatoire avec certificat non public). Fournisseur plateforme :
/// `postgresql-addon`.
pub const DOCUMENTDB: &str = "documentdb";

/// Fournisseur plateforme derrière un moteur interne.
pub fn platform_provider(provider: &str) -> &str {
    if provider == DOCUMENTDB { "postgresql-addon" } else { provider }
}

/// Paramètres de connexion que l'endpoint DocumentDB impose.
pub const DOCUMENTDB_URI_PARAMS: &str = "tls=true&tlsAllowInvalidCertificates=true";

/// Un service `mongo` va vers DocumentDB sauf choix contraire : l'addon
/// MongoDB n'existe qu'en 4.0.3, DocumentDB annonce 7.0 et accepte les
/// pilotes 5.0+. Un tag 4.x désigne l'addon MongoDB.
pub fn documentdb_by_default(rec: &Recognized) -> bool {
    rec.provider == "mongodb-addon" && rec.version.as_deref().and_then(|v| v.split('.').next()?.parse::<u32>().ok()).is_none_or(|major| major >= 5)
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
    // DocumentDB remplace un service `mongo` : mêmes variables déclarées.
    let provider = if r.provider == DOCUMENTDB { "mongodb-addon" } else { r.provider };
    match (provider, r.dialect) {
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
        (DOCUMENTDB, Role::Host) => "DOCUMENTDB_ADDON_HOST",
        (DOCUMENTDB, Role::Port) => "DOCUMENTDB_ADDON_PORT",
        (DOCUMENTDB, Role::User) => "POSTGRESQL_ADDON_USER",
        (DOCUMENTDB, Role::Password) => "POSTGRESQL_ADDON_PASSWORD",
        (DOCUMENTDB, Role::Database) => "POSTGRESQL_ADDON_DB",
        (DOCUMENTDB, Role::Uri) => "DOCUMENTDB_ADDON_URI",
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
/// Schéma que l'addon parle réellement, quand celui de l'URL d'origine ne
/// peut pas tenir (`rediss://`, `mongodb+srv://`...). `None` = conservé.
pub fn served_scheme(provider: &str, original: &str) -> Option<&'static str> {
    match provider {
        "redis-addon" if original != "redis" => Some("redis"),
        "mongodb-addon" | DOCUMENTDB if original != "mongodb" => Some("mongodb"),
        _ => None,
    }
}

pub fn rebuild_url(original: &str, addon: &str, provider: &str, private_net: Option<&str>) -> Option<Template> {
    let (scheme, rest) = original.split_once("://")?;
    let scheme = served_scheme(provider, scheme).unwrap_or(scheme);
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (_authority, tail) = rest.split_at(authority_end);
    let mut segs: Vec<Segment> = vec![Segment::Literal(format!("{scheme}://"))];
    let lit = |segs: &mut Vec<Segment>, s: &str| segs.push(Segment::Literal(s.to_string()));
    let push = |segs: &mut Vec<Segment>, s: Option<Segment>| {
        if let Some(s) = s {
            segs.push(s);
        }
    };
    // Addon privé : hôte = FQDN de membre, port fixe ; identifiants et base
    // restent ceux de la liaison.
    let host_port = |segs: &mut Vec<Segment>| match private_net {
        Some(net) => {
            segs.push(Segment::Ref(Reference::MemberFqdn { member: ResourceId(format!("member/{addon}@{net}")) }));
            segs.push(Segment::Literal(format!(":{}", addon_ng_port(provider))));
        }
        None => {
            push(segs, seg(addon, provider, Role::Host));
            if provider != "es-addon" {
                push(segs, Some(Segment::Literal(":".into())));
                push(segs, seg(addon, provider, Role::Port));
            }
        }
    };
    match provider {
        "redis-addon" => {
            // `redis://default:<password>@<host>:<port>` (doc Clever pour Docs).
            lit(&mut segs, "default:");
            push(&mut segs, seg(addon, provider, Role::Password));
            lit(&mut segs, "@");
            host_port(&mut segs);
            lit(&mut segs, tail);
        }
        "es-addon" => {
            // Public : HTTPS seul sur 443 (`ES_ADDON_HOST` est un hôte nu,
            // constaté le 13/09/2026). Dans le groupe : HTTP clair sur 8080
            // (`deploytypes/elasticsearch.rb`, vérifié le 16/09/2026).
            segs[0] = Segment::Literal(if private_net.is_some() { "http://".into() } else { "https://".into() });
            push(&mut segs, seg(addon, provider, Role::User));
            lit(&mut segs, ":");
            push(&mut segs, seg(addon, provider, Role::Password));
            lit(&mut segs, "@");
            host_port(&mut segs);
            lit(&mut segs, tail);
        }
        _ => {
            push(&mut segs, seg(addon, provider, Role::User));
            lit(&mut segs, ":");
            push(&mut segs, seg(addon, provider, Role::Password));
            lit(&mut segs, "@");
            host_port(&mut segs);
            // Le chemin est la base : celle de l'addon.
            let mut query = tail.find(['?', '#']).map(|i| tail[i..].to_string()).unwrap_or_default();
            if provider == DOCUMENTDB {
                // TLS obligatoire, certificat hors des autorités publiques.
                for param in DOCUMENTDB_URI_PARAMS.split('&') {
                    let key = param.split('=').next().unwrap_or("");
                    if !query.to_ascii_lowercase().contains(&format!("{}=", key.to_ascii_lowercase())) {
                        query.push(if query.is_empty() { '?' } else { '&' });
                        query.push_str(param);
                    }
                }
            }
            lit(&mut segs, "/");
            push(&mut segs, seg(addon, provider, Role::Database));
            if !query.is_empty() {
                lit(&mut segs, &query);
            }
        }
    }
    Some(Template(segs))
}

/// Plan retenu : `wanted` (slug) s'il existe, sinon le moins cher servi
/// dans la région.
/// Fournisseurs dont l'accès par le network group est vérifié (port
/// [`addon_ng_port`], identifiants de la liaison), vérifié le 16/09/2026
/// pour chacun.
pub const NG_VERIFIED: &[&str] = &["postgresql-addon", "mysql-addon", "redis-addon", "mongodb-addon", "es-addon", DOCUMENTDB];

/// Plan retenu : `wanted` (slug) s'il existe, sinon le moins cher servi
/// dans la région — dédié si l'addon doit être membre d'un network group
/// (un plan partagé n'a pas d'instance, donc pas de peer).
pub fn choose_plan<'a>(provider: &'a Provider, region: &str, wanted: Option<&str>, dedicated: bool) -> Result<&'a PlanInfo, String> {
    if let Some(w) = wanted {
        return provider.plans.iter().find(|p| p.slug == w || p.name.eq_ignore_ascii_case(w)).ok_or_else(|| format!("plan `{w}` inconnu ; disponibles : {}", provider.plans.iter().map(|p| p.slug.as_str()).collect::<Vec<_>>().join(", ")));
    }
    provider
        .plans
        .iter()
        .filter(|p| p.zones.is_empty() || p.zones.iter().any(|z| z == region))
        .filter(|p| !dedicated || !is_shared(p))
        .min_by(|a, b| a.price.partial_cmp(&b.price).unwrap())
        .ok_or_else(|| format!("aucun plan{} de {} dans la région {region}", if dedicated { " dédié" } else { "" }, provider.name))
}

/// Plan partagé (cluster mutualisé, pas d'instance dédiée) : la clé
/// s'écrit `Type` chez PostgreSQL et `TYPE` chez MySQL.
pub fn is_shared(plan: &PlanInfo) -> bool {
    plan.features.iter().any(|(k, v)| k.eq_ignore_ascii_case("type") && v.eq_ignore_ascii_case("shared"))
}

/// Limite de connexions d'un plan, si le catalogue la donne.
pub fn max_connections(plan: &PlanInfo) -> Option<u32> {
    plan.features.get("Max connection limit").and_then(|v| v.trim().parse().ok())
}
