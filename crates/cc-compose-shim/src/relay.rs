//! Ce que le shim fait du port plateforme (8080), voir `PlatformPort`.
//!
//! `CC_DOCKER_EXPOSED_HTTP_PORT` est interdit (CLAUDE.md §3), donc une image
//! HTTP qui écoute ailleurs est relayée ici, et une image non HTTP reçoit un
//! répondeur de santé, le déployeur validant par un GET HTTP. Dans les deux
//! cas 8080 n'ouvre qu'une fois le port réel joignable.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use cc_compose_model::shim::{PlatformPort, PLATFORM_HTTP_PORT};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const READY_POLL: Duration = Duration::from_millis(500);

fn local(port: u16) -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
}

pub async fn serve(port: PlatformPort) {
    let listen = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, PLATFORM_HTTP_PORT));
    let (forward_to, ready) = match port {
        PlatformPort::Image => return,
        PlatformPort::Forward { target_port } => (Some(local(target_port)), Some(local(target_port))),
        PlatformPort::Health { ready_port } => (None, ready_port.map(local)),
    };

    if let Some(ready) = ready {
        wait_ready(ready).await;
    }

    let listener = match TcpListener::bind(listen).await {
        Ok(l) => l,
        Err(e) => {
            // Un échec ici est fatal pour le déploiement : la plateforme ne
            // verra jamais 8080. On le dit clairement plutôt que d'échouer en
            // silence, et on laisse l'enfant vivre pour que les logs sortent.
            eprintln!("cc-shim: bind {listen} : {e}");
            return;
        }
    };
    match forward_to {
        Some(t) => eprintln!("cc-shim: {listen} relayé vers {t}"),
        None => eprintln!("cc-shim: {listen} répondeur de santé"),
    }

    loop {
        match listener.accept().await {
            Ok((inbound, _)) => match forward_to {
                Some(target) => {
                    tokio::spawn(forward(inbound, target));
                }
                None => {
                    tokio::spawn(health(inbound));
                }
            },
            Err(e) => {
                eprintln!("cc-shim: accept : {e}");
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

/// Répondeur de santé pour un service non HTTP : consomme la requête jusqu'à
/// la fin des en-têtes (bornée), répond `200 OK`, ferme.
async fn health(mut inbound: TcpStream) {
    let mut buf = [0u8; 4096];
    let mut read = 0;
    let deadline = tokio::time::sleep(Duration::from_secs(5));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            n = inbound.read(&mut buf[read..]) => match n {
                Ok(0) => break,
                Ok(n) => {
                    read += n;
                    if buf[..read].windows(4).any(|w| w == b"\r\n\r\n") || read == buf.len() {
                        break;
                    }
                }
                Err(_) => return,
            },
            _ = &mut deadline => return,
        }
    }
    let _ = inbound
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 3\r\nConnection: close\r\n\r\nok\n")
        .await;
    let _ = inbound.shutdown().await;
}
