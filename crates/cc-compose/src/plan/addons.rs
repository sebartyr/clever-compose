//! Substitution des services stateful par des addons managés (CLAUDE.md §6).
//!
//! Reconnaissance par image (fournisseur cible **et** dialecte de variables
//! du service source), version depuis le tag, plan le moins cher de la
//! région sauf `x-clever.plan`, coût annoncé. Câblage des identifiants des
//! consommateurs : URL reconstruite, variables discrètes corrélées par
//! égalité de valeur, formats non standards ; tout passe par des références
//! `AddonEnv` expansées par le shim au démarrage (`CLEVER_COMPOSE_EXPAND`),
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

/// Règles de reconnaissance des images et de lecture de leurs identifiants
/// (`data/addon-images.json`, provenance dans `data/PROVENANCE.md`).
#[derive(Debug, Deserialize)]
pub struct Images {
    pub dialects: indexmap::IndexMap<String, Dialect>,
    /// Nom d'image (sans `docker.io/library/`) vers dialecte.
    pub images: indexmap::IndexMap<String, String>,
}

/// Comment une image se configure : fournisseur cible, port, et où lire
/// chaque identifiant dans le service source.
#[derive(Debug, Deserialize)]
pub struct Dialect {
    pub provider: String,
    /// Nom court pour les messages (`postgres`, `mariadb`...).
    pub kind: String,
    pub port: u16,
    /// `major` ou `major.minor` : ce qu'on retient du tag.
    pub version: String,
    #[serde(default)]
    pub user: Option<CredentialSource>,
    #[serde(default)]
    pub password: Option<CredentialSource>,
    #[serde(default)]
    pub database: Option<CredentialSource>,
    #[serde(default)]
    pub root_password: Option<CredentialSource>,
}

/// Source d'un identifiant dans le service source.
#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct CredentialSource {
    /// Variables d'environnement, la première renseignée l'emporte.
    #[serde(default)]
    pub env: Vec<String>,
    /// Valeur par défaut de l'image.
    #[serde(default)]
    pub default: Option<String>,
    /// Valeur par défaut égale à un autre rôle (`POSTGRES_DB` = `POSTGRES_USER`).
    #[serde(default)]
    pub default_from: Option<String>,
    /// Valeur qui suit ce drapeau dans `command` (`--requirepass`).
    #[serde(default)]
    pub command_flag: Option<String>,
    /// Valeur imposée par l'image (`elastic`).
    #[serde(default)]
    pub fixed: Option<String>,
}

pub fn images() -> &'static Images {
    static I: OnceLock<Images> = OnceLock::new();
    I.get_or_init(|| serde_json::from_str(include_str!("../../data/addon-images.json")).expect("data/addon-images.json invalide"))
}

pub fn dialect(name: &str) -> Option<&'static Dialect> {
    images().dialects.get(name)
}

#[derive(Debug, Clone)]
pub struct Recognized {
    pub provider: &'static str,
    pub dialect: &'static Dialect,
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

/// Reconnaissance d'une image par son nom (§6.1), ou par le dialecte que
/// le service désigne (`x-clever.dialect`, image custom). Jamais un
/// service `build:` sans dialecte explicite.
pub fn recognize(image: &str) -> Option<Recognized> {
    let (name, tag) = split_image(image);
    let name = name.trim_start_matches("docker.io/").trim_start_matches("library/");
    let d = dialect(images().images.get(name)?)?;
    Some(recognized(d, tag))
}

pub fn recognize_as(dialect_name: &str, image: Option<&str>) -> Option<Recognized> {
    let d = dialect(dialect_name)?;
    Some(recognized(d, image.and_then(|i| split_image(i).1)))
}

fn split_image(image: &str) -> (&str, Option<&str>) {
    image.rsplit_once('@').map(|(n, _)| (n, None)).unwrap_or_else(|| match image.rsplit_once(':') {
        Some((n, t)) if !t.contains('/') => (n, Some(t)),
        _ => (image, None),
    })
}

fn recognized(d: &'static Dialect, tag: Option<&str>) -> Recognized {
    let major_only = d.version == "major";
    let version = tag.filter(|t| *t != "latest").and_then(|t| {
        let digits: String = t.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        if digits.is_empty() {
            return None;
        }
        let parts: Vec<&str> = digits.trim_end_matches('.').split('.').collect();
        Some(if major_only { parts[0].to_string() } else { parts.iter().take(2).cloned().collect::<Vec<_>>().join(".") })
    });
    Recognized { provider: &d.provider, dialect: d, kind: &d.kind, version, port: d.port }
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
    let d = r.dialect;
    let read = |src: &Option<CredentialSource>| -> Option<String> {
        let src = src.as_ref()?;
        if let Some(f) = &src.fixed {
            return Some(f.clone());
        }
        if let Some(flag) = &src.command_flag {
            let cmd = s.command.clone().unwrap_or_default();
            return cmd.iter().position(|a| a == flag).and_then(|i| cmd.get(i + 1).cloned());
        }
        src.env.iter().find_map(|k| s.environment.get(k).and_then(|v| v.clone()).filter(|v| !v.is_empty())).or_else(|| src.default.clone())
    };
    let mut out = Declared { user: read(&d.user), password: read(&d.password), database: read(&d.database), root_password: read(&d.root_password) };
    // `default_from` : un rôle vide prend la valeur d'un autre (une seule
    // passe, un rôle ne dépend que d'un rôle lu directement).
    let from = |role: &str, out: &Declared| match role {
        "user" => out.user.clone(),
        "password" => out.password.clone(),
        "database" => out.database.clone(),
        "root_password" => out.root_password.clone(),
        _ => None,
    };
    if out.database.is_none() {
        if let Some(r) = d.database.as_ref().and_then(|c| c.default_from.as_deref()) {
            out.database = from(r, &out);
        }
    }
    if out.user.is_none() {
        if let Some(r) = d.user.as_ref().and_then(|c| c.default_from.as_deref()) {
            out.user = from(r, &out);
        }
    }
    out
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
