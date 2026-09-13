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

pub fn rewrite(key: &str, value: &str, consumer: &str, consumer_service: &Service, project: &Project, memberships: &Memberships, p: &Path, diags: &mut Diagnostics, referenced: &mut std::collections::BTreeSet<String>) -> Template {
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
            if let Some((service, net)) = lookup(host, consumer, consumer_service, project, memberships, p, diags) {
                referenced.insert(service.clone());
                literal.push_str(before);
                literal.push_str(&authority[..userinfo_len]);
                segments.push(Segment::Literal(std::mem::take(&mut literal)));
                segments.push(member_ref(&service, &net));
                rest = &after[userinfo_len + host.len()..];
                continue;
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
            if let Some((service, net)) = lookup(token, consumer, consumer_service, project, memberships, p, diags) {
                referenced.insert(service.clone());
                segments.push(Segment::Literal(std::mem::take(&mut literal)));
                segments.push(member_ref(&service, &net));
                rest = &rest[token_end..];
                continue;
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

fn lookup(host: &str, consumer: &str, consumer_service: &Service, project: &Project, memberships: &Memberships, p: &Path, diags: &mut Diagnostics) -> Option<(String, String)> {
    if host == consumer {
        return None;
    }
    if let Some(found) = resolve_alias(host, consumer, consumer_service, memberships) {
        return Some(found);
    }
    // Un nom de service que le consommateur n'atteint par aucun réseau.
    if project.services.contains_key(host) {
        diags.push(Diagnostic::warning("service-unreachable", format!("`{host}` est un service du projet mais `{consumer}` ne partage aucun réseau avec lui : la valeur est laissée telle quelle")).with_path(p));
    }
    None
}

/// `CC_COMPOSE_DNS_MAP=alias=<fqdn>,...` : les alias des services que ce
/// service référence (`depends_on`, hôtes de son environnement) et qu'il
/// atteint par un réseau commun, triés. Restreindre aux références rend le
/// graphe des ressources acyclique, donc l'export Terraform applicable en
/// une passe (summary.md, phase 0 question 2).
pub fn dns_map(consumer: &str, consumer_service: &Service, memberships: &Memberships, referenced: &std::collections::BTreeSet<String>) -> Template {
    let mut entries: Vec<(String, String, String)> = Vec::new();
    for net in consumer_service.networks.keys() {
        if let Some(members) = memberships.get(net) {
            for (service, aliases) in members {
                if service == consumer || !referenced.contains(service) {
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
