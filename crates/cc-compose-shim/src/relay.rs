//! Relais TCP du port plateforme (8080) vers le port réel de l'image.
//!
//! `CC_DOCKER_EXPOSED_HTTP_PORT` est interdit (CLAUDE.md §3), donc une image
//! qui écoute ailleurs est relayée ici. Le relais attend que le port réel
//! accepte une connexion avant d'ouvrir 8080.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use cc_compose_model::shim::PLATFORM_HTTP_PORT;
use tokio::net::{TcpListener, TcpStream};

const READY_POLL: Duration = Duration::from_millis(500);

pub async fn serve(target_port: u16) {
    let target = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, target_port));
    let listen = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, PLATFORM_HTTP_PORT));

    wait_ready(target).await;

    let listener = match TcpListener::bind(listen).await {
        Ok(l) => l,
        Err(e) => {
            // Un échec ici est fatal pour le déploiement : la plateforme ne
            // verra jamais 8080. On le dit clairement plutôt que d'échouer en
            // silence, et on laisse l'enfant vivre pour que les logs sortent.
            eprintln!("cc-shim: relais : bind {listen} : {e}");
            return;
        }
    };
    eprintln!("cc-shim: relais {listen} -> {target}");

    loop {
        match listener.accept().await {
            Ok((inbound, _)) => {
                tokio::spawn(forward(inbound, target));
            }
            Err(e) => {
                eprintln!("cc-shim: relais : accept : {e}");
                tokio::time::sleep(READY_POLL).await;
            }
        }
    }
}

async fn wait_ready(target: SocketAddr) {
    loop {
        match TcpStream::connect(target).await {
            Ok(_) => return,
            Err(_) => tokio::time::sleep(READY_POLL).await,
        }
    }
}

async fn forward(mut inbound: TcpStream, target: SocketAddr) {
    let mut outbound = match TcpStream::connect(target).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cc-shim: relais : connexion {target} : {e}");
            return;
        }
    };
    let _ = inbound.set_nodelay(true);
    let _ = outbound.set_nodelay(true);
    let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
}
