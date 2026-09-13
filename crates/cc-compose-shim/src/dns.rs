//! Stub DNS (CLAUDE.md §8) pour les noms de services codés en dur dans les
//! images (`proxy_pass http://app:8000`). Table `CC_COMPOSE_DNS_MAP`
//! (`alias=fqdn,...`). Règles :
//!
//! - pas de cache : chaque requête est résolue en amont et garde son TTL
//!   (le résolveur de la VM cache déjà ; un cache local survivant à un
//!   blue/green renverrait des IPs mortes) ;
//! - les serveurs amont sont ceux du `resolv.conf` d'origine, lu avant sa
//!   réécriture **en place** (bind mount : pas de renommage) sans `search`,
//!   pour que `app` arrive sous la forme exacte `app.` ;
//! - écoute sur 127.0.0.153:53 (jamais 127.0.0.53), `EADDRINUSE` explicite ;
//! - nom de la table : CNAME vers le FQDN + les enregistrements résolus en
//!   amont pour le FQDN ; une AAAA laisse passer la réponse amont (NODATA
//!   compris), un NXDOMAIN casserait les clients qui tentent AAAA puis A ;
//! - autre nom : relayé tel quel.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use hickory_proto::op::{Message, MessageType, ResponseCode};
use hickory_proto::rr::rdata::CNAME;
use hickory_proto::rr::{Name, RData, Record};
use tokio::net::UdpSocket;

pub const LISTEN: Ipv4Addr = Ipv4Addr::new(127, 0, 0, 153);
pub const MAP_VAR: &str = "CC_COMPOSE_DNS_MAP";
const RESOLV: &str = "/etc/resolv.conf";

pub struct Stub {
    table: HashMap<Name, Name>,
    upstream: Vec<SocketAddr>,
}

/// Nom DNS absolu depuis un texte, label par label : les identifiants de la
/// plateforme contiennent des `_` (`app_…`, `ng_…`) que le parseur de noms
/// d'hôte de hickory refuse.
pub fn parse_name(text: &str) -> Option<Name> {
    let text = text.trim().trim_end_matches('.');
    if text.is_empty() {
        return None;
    }
    let mut name = Name::from_labels(text.split('.').map(|l| l.as_bytes().to_vec())).ok()?;
    name.set_fqdn(true);
    Some(name)
}

/// Table depuis l'environnement, vide si absente.
pub fn table() -> HashMap<Name, Name> {
    let mut out = HashMap::new();
    let Some(raw) = std::env::var_os(MAP_VAR) else { return out };
    for entry in raw.to_string_lossy().split(',') {
        let Some((alias, fqdn)) = entry.split_once('=') else { continue };
        match (parse_name(alias), parse_name(fqdn)) {
            (Some(a), Some(f)) => {
                out.insert(a.to_lowercase(), f);
            }
            _ => eprintln!("cc-shim: dns : entrée `{entry}` ignorée"),
        }
    }
    out
}

/// Serveurs amont du `resolv.conf` d'origine.
fn upstream_from_resolv() -> Vec<SocketAddr> {
    let text = std::fs::read_to_string(RESOLV).unwrap_or_default();
    text.lines()
        .filter_map(|l| l.split_whitespace().nth(1).filter(|_| l.trim_start().starts_with("nameserver")))
        .filter_map(|ip| ip.parse::<IpAddr>().ok())
        .filter(|ip| *ip != IpAddr::V4(LISTEN))
        .map(|ip| SocketAddr::new(ip, 53))
        .collect()
}

/// Démarre le stub et réécrit `resolv.conf`. Sans table, ne fait rien.
pub async fn start() -> Result<(), String> {
    let table = table();
    if table.is_empty() {
        return Ok(());
    }
    let mut upstream = upstream_from_resolv();
    if upstream.is_empty() {
        upstream.push(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 53));
        eprintln!("cc-shim: dns : aucun nameserver dans {RESOLV}, amont 1.1.1.1");
    }
    let socket = UdpSocket::bind(SocketAddr::new(IpAddr::V4(LISTEN), 53)).await.map_err(|e| match e.kind() {
        std::io::ErrorKind::AddrInUse => format!("dns : {LISTEN}:53 déjà utilisé (EADDRINUSE) : un autre résolveur tourne dans le conteneur"),
        _ => format!("dns : bind {LISTEN}:53 : {e}"),
    })?;
    // Réécriture en place : tronquer puis écrire (le fichier est un bind
    // mount, `rm` est refusé).
    let mut content = format!("# Réécrit par cc-shim : stub DNS pour les noms de services.\nnameserver {LISTEN}\noptions ndots:1\n");
    content.push_str(&format!("# amont : {}\n", upstream.iter().map(|u| u.ip().to_string()).collect::<Vec<_>>().join(" ")));
    std::fs::write(RESOLV, content).map_err(|e| format!("dns : réécriture de {RESOLV} : {e}"))?;
    eprintln!("cc-shim: dns : {} nom(s) sur {LISTEN}, amont {:?}", table.len(), upstream.iter().map(|u| u.ip()).collect::<Vec<_>>());
    let stub = Arc::new(Stub { table, upstream });
    let socket = Arc::new(socket);
    tokio::spawn(async move {
        let mut buf = vec![0u8; 4096];
        loop {
            let Ok((n, from)) = socket.recv_from(&mut buf).await else { continue };
            let data = buf[..n].to_vec();
            let (stub, socket) = (stub.clone(), socket.clone());
            tokio::spawn(async move {
                if let Some(reply) = stub.handle(&data).await {
                    let _ = socket.send_to(&reply, from).await;
                }
            });
        }
    });
    Ok(())
}

impl Stub {
    async fn forward(&self, data: &[u8]) -> Option<Vec<u8>> {
        for up in &self.upstream {
            let Ok(sock) = UdpSocket::bind("0.0.0.0:0").await else { continue };
            if sock.send_to(data, up).await.is_err() {
                continue;
            }
            let mut buf = vec![0u8; 4096];
            match tokio::time::timeout(Duration::from_secs(3), sock.recv_from(&mut buf)).await {
                Ok(Ok((n, _))) => return Some(buf[..n].to_vec()),
                _ => continue,
            }
        }
        None
    }

    async fn handle(&self, data: &[u8]) -> Option<Vec<u8>> {
        let query = Message::from_vec(data).ok()?;
        let Some(q) = query.queries().first() else {
            return self.forward(data).await;
        };
        let asked = q.name().to_lowercase();
        let Some(fqdn) = self.table.get(&asked) else {
            return self.forward(data).await;
        };
        // Même question, sur le FQDN, en amont.
        let mut upstream_q = query.clone();
        upstream_q.queries_mut()[0].set_name(fqdn.clone());
        let raw = self.forward(&upstream_q.to_vec().ok()?).await;
        let mut reply = Message::new();
        reply.set_id(query.id()).set_message_type(MessageType::Response).set_recursion_desired(query.recursion_desired()).set_recursion_available(true);
        reply.add_query(q.clone());
        match raw.and_then(|r| Message::from_vec(&r).ok()) {
            Some(up) => {
                reply.set_response_code(if up.response_code() == ResponseCode::NXDomain { ResponseCode::NoError } else { up.response_code() });
                // CNAME alias -> fqdn, TTL du premier enregistrement amont.
                let ttl = up.answers().first().map(|r| r.ttl()).unwrap_or(30);
                reply.add_answer(Record::from_rdata(q.name().clone(), ttl, RData::CNAME(CNAME(fqdn.clone()))));
                for r in up.answers() {
                    reply.add_answer(r.clone());
                }
            }
            None => {
                reply.set_response_code(ResponseCode::ServFail);
            }
        }
        reply.to_vec().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_with_underscores_are_accepted() {
        let n = parse_name("app_9e640c6b-ef85-4d78-9b42-2ecf30463806.m.ng_43874858-aadd-4704-8da4-b44e5851ac2b.cc-ng.cloud").unwrap();
        assert_eq!(n.to_ascii(), "app_9e640c6b-ef85-4d78-9b42-2ecf30463806.m.ng_43874858-aadd-4704-8da4-b44e5851ac2b.cc-ng.cloud.");
        assert!(parse_name("app").is_some());
    }
}
