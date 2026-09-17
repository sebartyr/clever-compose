//! Ce que le shim fait du port plateforme (8080), voir `PlatformPort`.
//!
//! `CC_DOCKER_EXPOSED_HTTP_PORT` est interdit (CLAUDE.md §3), donc une image
//! HTTP qui écoute ailleurs est relayée ici, et une image non HTTP reçoit un
//! répondeur de santé, le déployeur validant par un GET HTTP. Dans les deux
//! cas 8080 n'ouvre qu'une fois le port réel joignable.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use std::collections::HashMap;
use std::sync::Arc;

use cc_compose_model::shim::{PlatformPort, TcpRelay, UdpRelay, PLATFORM_HTTP_PORT, PLATFORM_TCP_PORT};
use tokio::net::UdpSocket;
use tokio::sync::Mutex;
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

/// Redirection TCP : relais brut de 4040 vers le port réel.
pub async fn serve_tcp(relay: TcpRelay) {
    let target = local(relay.target_port);
    let listen = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, PLATFORM_TCP_PORT));
    wait_ready(target).await;
    let listener = match TcpListener::bind(listen).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cc-shim: bind {listen} : {e}");
            return;
        }
    };
    eprintln!("cc-shim: {listen} relayé vers {target} (tcp)");
    loop {
        match listener.accept().await {
            Ok((inbound, _)) => {
                tokio::spawn(forward(inbound, target));
            }
            Err(e) => {
                eprintln!("cc-shim: accept : {e}");
                tokio::time::sleep(READY_POLL).await;
            }
        }
    }
}

/// Sources UDP suivies au plus : 4040 est public, des sources forgées ne
/// doivent pas épuiser sockets et mémoire. Au-delà, les nouvelles sources
/// sont ignorées jusqu'à expiration d'entrées.
const UDP_MAX_SOURCES: usize = 4096;
/// Une source sans trafic retour depuis ce délai est oubliée.
const UDP_IDLE: Duration = Duration::from_secs(60);

/// Redirection UDP : chaque source distante reçoit son propre socket vers la
/// cible, pour que les réponses reviennent à la bonne source. Une entrée
/// inactive depuis [`UDP_IDLE`] est fermée ; au plus [`UDP_MAX_SOURCES`].
pub async fn serve_udp(relay: UdpRelay) {
    let target = local(relay.target_port);
    let listen = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, PLATFORM_TCP_PORT));
    let inbound = match UdpSocket::bind(listen).await {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln!("cc-shim: bind udp {listen} : {e}");
            return;
        }
    };
    eprintln!("cc-shim: {listen} relayé vers {target} (udp)");
    let table: Arc<Mutex<HashMap<SocketAddr, Arc<UdpSocket>>>> = Arc::new(Mutex::new(HashMap::new()));
    let mut buf = vec![0u8; 65535];
    loop {
        let (n, from) = match inbound.recv_from(&mut buf).await {
            Ok(x) => x,
            Err(e) => {
                eprintln!("cc-shim: udp recv : {e}");
                continue;
            }
        };
        let sock = {
            let mut t = table.lock().await;
            match t.get(&from) {
                Some(s) => s.clone(),
                None => {
                    if t.len() >= UDP_MAX_SOURCES {
                        continue;
                    }
                    let Ok(s) = UdpSocket::bind("127.0.0.1:0").await else { continue };
                    if s.connect(target).await.is_err() {
                        continue;
                    }
                    let s = Arc::new(s);
                    t.insert(from, s.clone());
                    // Réponses de la cible vers la source, puis expiration.
                    let (inbound, table, s2) = (inbound.clone(), table.clone(), s.clone());
                    tokio::spawn(async move {
                        let mut rbuf = vec![0u8; 65535];
                        // Réponses vers le client d'origine, jusqu'à UDP_IDLE sans trafic.
                        while let Ok(Ok(n)) = tokio::time::timeout(UDP_IDLE, s2.recv(&mut rbuf)).await {
                            let _ = inbound.send_to(&rbuf[..n], from).await;
                        }
                        table.lock().await.remove(&from);
                    });
                    s
                }
            }
        };
        let _ = sock.send(&buf[..n]).await;
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
