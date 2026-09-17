//! Réécriture statique des variables d'environnement (CLAUDE.md §7) : un nom
//! de service utilisé comme hôte devient une référence au FQDN du membre.
//! Reconnaît l'hôte d'une URL (`scheme://[user:pass@]host[:port]`), un
//! motif `host:port`, ou la valeur entière égale au nom.

use cc_compose_model::plan::{Reference, ResourceId, Segment, Template};
use indexmap::IndexMap;

use crate::compose::model::{Project, Service};
use crate::compose::node::Path;
use crate::diagnostic::{Diagnostic, Diagnostics};

type Memberships = IndexMap<String, Vec<(String, Vec<String>)>>;

/// Un service substitué par un addon.
#[derive(Debug, Clone, Copy)]
pub struct AddonRef {
    pub provider: &'static str,
    /// Membre du network group : joint par son FQDN de membre sur
    /// `addon_ng_port`, sinon par son hôte public.
    pub private: bool,
}

/// Services substitués par un addon : nom → fournisseur et mode d'accès.
pub type Addons = IndexMap<String, AddonRef>;

/// Ce qu'un nom d'hôte désigne.
enum Target {
    Member(String, String),
    /// Addon public : variables `*_ADDON_HOST`/`_PORT` de la liaison.
    Addon(String, &'static str),
    /// Addon privé atteint par le réseau `net` : FQDN de membre, port fixe.
    PrivateAddon(String, &'static str, String),
}

/// Le réseau par lequel `consumer` atteint l'addon privé `addon`, s'il y en
/// a un.
pub fn shared_net(addon: &str, consumer_service: &Service, memberships: &Memberships) -> Option<String> {
    consumer_service.networks.keys().find(|net| memberships.get(*net).is_some_and(|m| m.iter().any(|(s, _)| s == addon))).cloned()
}

/// Le membre (`member/<service>@<réseau>`) que `consumer` atteint sous
/// `alias`, s'ils partagent un réseau.
fn resolve_alias(alias: &str, consumer: &str, consumer_service: &Service, memberships: &Memberships) -> Option<(String, String)> {
    for net in consumer_service.networks.keys() {
        if let Some(members) = memberships.get(net) {
            for (service, aliases) in members {
                if service != consumer && aliases.iter().any(|a| a == alias) {
                    return Some((service.clone(), net.clone()));
                }
            }
        }
    }
    None
}

fn member_ref(service: &str, net: &str) -> Segment {
    Segment::Ref(Reference::MemberFqdn { member: ResourceId(format!("member/{service}@{net}")) })
}

fn is_host_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'
}

/// Une clé qui désigne vraisemblablement un hôte : seule forme où une
/// valeur entière égale à un nom de service est réécrite (`DB_HOST=db`),
/// pour ne pas prendre `MYSQL_USER=wordpress` pour une adresse.
fn key_names_a_host(key: &str) -> bool {
    let k = key.to_ascii_uppercase();
    ["HOST", "SERVER", "ADDR", "ENDPOINT", "HOSTNAME"].iter().any(|h| k.contains(h))
}

#[allow(clippy::too_many_arguments)]
pub fn rewrite(key: &str, value: &str, consumer: &str, consumer_service: &Service, project: &Project, memberships: &Memberships, addons: &Addons, p: &Path, diags: &mut Diagnostics, referenced: &mut std::collections::BTreeSet<String>) -> Template {
    let mut segments: Vec<Segment> = Vec::new();
    let mut literal = String::new();
    let mut rest = value;
    while !rest.is_empty() {
        // Hôte d'une URL.
        if let Some(pos) = rest.find("://") {
            let (before, after) = rest.split_at(pos + 3);
            let authority_end = after.find(['/', '?', '#']).unwrap_or(after.len());
            let authority = &after[..authority_end];
            let hostport = authority.rsplit('@').next().unwrap_or(authority);
            let host = hostport.split(':').next().unwrap_or(hostport);
            let userinfo_len = authority.len() - hostport.len();
            match lookup(host, consumer, consumer_service, project, memberships, addons, p, diags) {
                Some(Target::Member(service, net)) => {
                    referenced.insert(service.clone());
                    literal.push_str(before);
                    literal.push_str(&authority[..userinfo_len]);
                    segments.push(Segment::Literal(std::mem::take(&mut literal)));
                    segments.push(member_ref(&service, &net));
                    rest = &after[userinfo_len + host.len()..];
                    continue;
                }
                Some(Target::Addon(service, provider)) | Some(Target::PrivateAddon(service, provider, _)) => {
                    // URL entière reconstruite depuis l'addon (§6.2, cas 1).
                    let private_net = match lookup(host, consumer, consumer_service, project, memberships, addons, p, &mut Diagnostics::default()) {
                        Some(Target::PrivateAddon(_, _, net)) => Some(net),
                        _ => None,
                    };
                    referenced.insert(service.clone());
                    let url_end = pos + 3 + after.len();
                    let url_start = literal.rfind(|c: char| c.is_whitespace() || c == ',' || c == ';').map(|i| i + 1).unwrap_or(0);
                    let prefix_in_literal = literal[url_start..].to_string();
                    let full = format!("{prefix_in_literal}{}", &rest[..url_end]);
                    literal.truncate(url_start);
                    let scheme = full.split("://").next().unwrap_or("");
                    if let Some(served) = super::addons::served_scheme(provider, scheme) {
                        diags.push(Diagnostic::warning("addon-scheme", format!("`{key}` : `{scheme}://` devient `{served}://`, le schéma que l'addon `{service}` sert")).with_path(p));
                    }
                    if let Some(t) = super::addons::rebuild_url(&full, &service, provider, private_net.as_deref()) {
                        if !literal.is_empty() {
                            segments.push(Segment::Literal(std::mem::take(&mut literal)));
                        }
                        segments.extend(t.0);
                        rest = &rest[url_end..];
                        continue;
                    }
                    literal.push_str(&prefix_in_literal);
                }
                None => {}
            }
            literal.push_str(before);
            literal.push_str(authority);
            rest = &after[authority_end..];
            continue;
        }
        // Valeur entière, ou motif `host:port` / `host` en tête ou après un
        // séparateur.
        let token_end = rest.find(|c: char| !is_host_char(c)).unwrap_or(rest.len());
        let token = &rest[..token_end];
        let boundary_before = literal.is_empty() || literal.ends_with(|c: char| !is_host_char(c));
        let followed_by_port = rest[token_end..].starts_with(':') && rest[token_end + 1..].starts_with(|c: char| c.is_ascii_digit());
        let whole = literal.is_empty() && token_end == rest.len() && key_names_a_host(key);
        if !token.is_empty() && boundary_before && (whole || followed_by_port) {
            match lookup(token, consumer, consumer_service, project, memberships, addons, p, diags) {
                Some(Target::Member(service, net)) => {
                    referenced.insert(service.clone());
                    segments.push(Segment::Literal(std::mem::take(&mut literal)));
                    segments.push(member_ref(&service, &net));
                    rest = &rest[token_end..];
                    continue;
                }
                Some(Target::Addon(service, provider)) => {
                    referenced.insert(service.clone());
                    segments.push(Segment::Literal(std::mem::take(&mut literal)));
                    if let Some(h) = super::addons::seg(&service, provider, super::addons::Role::Host) {
                        segments.push(h);
                    }
                    rest = &rest[token_end..];
                    // `host:port` : le port devient celui de l'addon.
                    if followed_by_port {
                        let port_end = rest[1..].find(|c: char| !c.is_ascii_digit()).map(|i| i + 1).unwrap_or(rest.len());
                        if let Some(pt) = super::addons::seg(&service, provider, super::addons::Role::Port) {
                            segments.push(Segment::Literal(":".into()));
                            segments.push(pt);
                            rest = &rest[port_end..];
                        }
                    }
                    continue;
                }
                Some(Target::PrivateAddon(service, provider, net)) => {
                    // Membre du groupe : FQDN de membre, port du fournisseur.
                    let port = cc_compose_model::plan::addon_ng_port(provider);
                    referenced.insert(service.clone());
                    segments.push(Segment::Literal(std::mem::take(&mut literal)));
                    segments.push(member_ref(&service, &net));
                    rest = &rest[token_end..];
                    if followed_by_port {
                        let port_end = rest[1..].find(|c: char| !c.is_ascii_digit()).map(|i| i + 1).unwrap_or(rest.len());
                        segments.push(Segment::Literal(format!(":{port}")));
                        rest = &rest[port_end..];
                    } else {
                        // Hôte seul : l'application supposera le port par défaut
                        // du protocole, pas celui du groupe.
                        diags.push(Diagnostic::warning("addon-private-port", format!("`{key}` reçoit le FQDN de `{service}` dans le network group, où l'addon écoute sur {port} et non sur son port par défaut ; donnez le port à l'application (`{key}={token}:{port}` si elle accepte `hôte:port`, ou sa variable de port), ou `x-clever.private: false`")).with_path(p));
                    }
                    continue;
                }
                None => {}
            }
        }
        if token.is_empty() {
            let c = rest.chars().next().unwrap();
            literal.push(c);
            rest = &rest[c.len_utf8()..];
        } else {
            literal.push_str(token);
            rest = &rest[token_end..];
        }
    }
    if !literal.is_empty() || segments.is_empty() {
        segments.push(Segment::Literal(literal));
    }
    // Fusionner les littéraux vides.
    segments.retain(|s| !matches!(s, Segment::Literal(l) if l.is_empty()));
    if segments.is_empty() {
        segments.push(Segment::Literal(String::new()));
    }
    Template(segments)
}

#[allow(clippy::too_many_arguments)]
fn lookup(host: &str, consumer: &str, consumer_service: &Service, project: &Project, memberships: &Memberships, addons: &Addons, p: &Path, diags: &mut Diagnostics) -> Option<Target> {
    if host == consumer {
        return None;
    }
    if let Some(a) = addons.get(host) {
        if a.private {
            if let Some(net) = shared_net(host, consumer_service, memberships) {
                return Some(Target::PrivateAddon(host.to_string(), a.provider, net));
            }
            diags.push(Diagnostic::warning("addon-private-unreachable", format!("`{host}` est un addon membre de ses réseaux mais `{consumer}` n'en partage aucun : joint par son hôte public")).with_path(p));
        }
        return Some(Target::Addon(host.to_string(), a.provider));
    }
    if let Some((service, net)) = resolve_alias(host, consumer, consumer_service, memberships) {
        return Some(Target::Member(service, net));
    }
    // Un nom de service que le consommateur n'atteint par aucun réseau.
    if project.services.contains_key(host) {
        diags.push(Diagnostic::warning("service-unreachable", format!("`{host}` est un service du projet mais `{consumer}` ne partage aucun réseau avec lui : la valeur est laissée telle quelle")).with_path(p));
    }
    None
}

#[allow(clippy::too_many_arguments)]
/// `CLEVER_COMPOSE_DNS_MAP=alias=<fqdn>,...` : les alias des services que ce
/// service référence (`depends_on`, hôtes de son environnement) et qu'il
/// atteint par un réseau commun, triés. Restreindre aux références rend le
/// graphe des ressources acyclique, donc l'export Terraform applicable en
/// une passe (summary.md, phase 0 question 2).
pub fn dns_map(consumer: &str, consumer_service: &Service, memberships: &Memberships, referenced: &std::collections::BTreeSet<String>, addons: &Addons) -> Template {
    let mut entries: Vec<(String, String, String)> = Vec::new();
    for net in consumer_service.networks.keys() {
        if let Some(members) = memberships.get(net) {
            for (service, aliases) in members {
                // Un addon public n'a pas de membre ; un addon privé en a un.
                if service == consumer || !referenced.contains(service) || addons.get(service).is_some_and(|a| !a.private) {
                    continue;
                }
                for a in aliases {
                    if !entries.iter().any(|(al, _, _)| al == a) {
                        entries.push((a.clone(), service.clone(), net.clone()));
                    }
                }
            }
        }
    }
    entries.sort();
    let mut segments = Vec::new();
    for (i, (alias, service, net)) in entries.iter().enumerate() {
        let prefix = if i == 0 { format!("{alias}=") } else { format!(",{alias}=") };
        segments.push(Segment::Literal(prefix));
        segments.push(member_ref(service, net));
    }
    Template(segments)
}
