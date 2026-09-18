//! cc-shim : PID 1 dans le conteneur de l'utilisateur.
//!
//! Version minimale de phase 0 (CLAUDE.md §8) : relais de port, signaux, exec.
//! Pas de stub DNS, pas de secrets, pas d'expansion de variables tant qu'un
//! test ne l'impose pas.
//!
//! Séquence : lire la config figée, lancer l'enfant avec les privilèges de
//! l'image, servir 8080 (relais ou santé) quand le port réel répond, relayer les signaux,
//! récupérer les zombies, propager le code de sortie (sémantique tini).

mod child;
mod dns;
mod expand;
mod relay;
mod secrets;
mod volumes;

use std::process::ExitCode;

use cc_compose_model::shim::{ShimConfig, CONFIG_PATH};

fn main() -> ExitCode {
    let config = match load_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("cc-shim: {e}");
            return ExitCode::from(125);
        }
    };

    let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("cc-shim: runtime tokio : {e}");
            return ExitCode::from(125);
        }
    };

    match rt.block_on(run(config)) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("cc-shim: {e}");
            ExitCode::from(125)
        }
    }
}

fn load_config() -> Result<ShimConfig, String> {
    let raw = std::fs::read(CONFIG_PATH).map_err(|e| format!("lecture de {CONFIG_PATH} : {e}"))?;
    let config: ShimConfig =
        serde_json::from_slice(&raw).map_err(|e| format!("{CONFIG_PATH} invalide : {e}"))?;
    if config.argv.is_empty() {
        return Err(format!("{CONFIG_PATH} : argv vide"));
    }
    Ok(config)
}

async fn run(config: ShimConfig) -> Result<u8, String> {
    // L'enfant démarre avant le relais : le relais n'ouvre 8080 qu'une fois le
    // port réel joignable, pour que le succès du déploiement vu par la
    // plateforme reflète la disponibilité réelle du service.
    // Le stub DNS et resolv.conf avant l'enfant, qui peut résoudre au
    // démarrage (nginx).
    dns::start().await?;
    volumes::prepare(&config)?;
    let consumed = secrets::materialize(&config.secrets)?;
    let expanded = expand::expand_all();
    let child = child::spawn(&config, &consumed, &expanded)?;

    tokio::spawn(relay::serve(config.port.clone()));
    if let Some(tcp) = config.tcp.clone() {
        tokio::spawn(relay::serve_tcp(tcp));
    }
    if let Some(udp) = config.udp.clone() {
        tokio::spawn(relay::serve_udp(udp));
    }

    child.supervise().await
}
