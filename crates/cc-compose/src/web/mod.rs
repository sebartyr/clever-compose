//! `clever-compose ui` : un serveur HTTP local qui sert l'interface (Vue +
//! @clevercloud/components, embarquée dans le binaire) et une API JSON qui
//! ne fait rien d'autre que ce que la CLI sait faire : mêmes plan, exécuteur
//! et identifiants d'environnement. Les opérations longues (`up`, `down`,
//! `seed`, `logs`) sont diffusées en SSE, une ligne du journal par
//! événement.
//!
//! Projets locaux : le répertoire de lancement, tout répertoire ajouté par
//! son chemin, ou une archive (`.zip`, `.tar.gz`, `.tgz`, `.tar`) envoyée
//! par l'interface et extraite dans un répertoire temporaire.

mod api;
mod assets;
mod local;

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

use crate::exec::platform::Api;

/// Identifiants et organisation courants : ceux de l'environnement au
/// lancement, ou saisis dans l'interface (`POST /api/session`). Rien n'est
/// écrit sur le disque.
pub struct Session {
    pub api: Api,
    pub org: String,
}

/// Consumer OAuth 1.0 (créé par l'utilisateur dans la console, base URL =
/// adresse d'écoute) : permet la danse OAuth quand aucun jeton n'est dans
/// l'environnement.
#[derive(Clone)]
pub struct OauthConsumer {
    pub key: String,
    pub secret: String,
}

pub struct State {
    pub session: RwLock<Option<Session>>,
    pub locals: local::Locals,
    pub oauth: Option<OauthConsumer>,
    /// `http://<adresse d'écoute>`, base du `oauth_callback`.
    pub public_base: String,
    /// Request tokens en attente d'autorisation → leur secret.
    pub pending: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

impl State {
    /// L'API de la session, ou une erreur si aucun identifiant n'est posé.
    pub fn api(&self) -> Result<Api, String> {
        self.session.read().unwrap().as_ref().map(|s| s.api.clone()).ok_or_else(|| "aucun identifiant : renseignez un jeton dans l'interface (ou CLEVER_TOKEN/CLEVER_SECRET, CC_API_TOKEN au lancement)".to_string())
    }
}

/// Lance le serveur et bloque jusqu'à Ctrl-C. `session` peut être absente :
/// l'interface demandera les identifiants.
pub async fn serve(listen: SocketAddr, session: Option<Session>, oauth: Option<OauthConsumer>, initial_dir: Option<std::path::PathBuf>, open_browser: bool) -> Result<(), String> {
    if session.is_none() && oauth.is_none() {
        return Err("aucun identifiant : posez CLEVER_TOKEN et CLEVER_SECRET (ou CC_API_TOKEN), ou un consumer OAuth (CLEVER_CONSUMER_KEY et CLEVER_CONSUMER_SECRET, base URL = adresse d'écoute) pour vous connecter depuis l'interface".into());
    }
    let locals = local::Locals::default();
    if let Some(dir) = initial_dir {
        if dir.join("compose.yaml").is_file() || dir.join("compose.yml").is_file() || dir.join("docker-compose.yaml").is_file() || dir.join("docker-compose.yml").is_file() {
            locals.add_dir(dir, false)?;
        }
    }
    let state = Arc::new(State { session: RwLock::new(session), locals, oauth, public_base: format!("http://{listen}"), pending: Default::default() });
    let app = api::router(state).fallback(assets::serve);
    let listener = tokio::net::TcpListener::bind(listen).await.map_err(|e| format!("écoute sur {listen} : {e}"))?;
    let url = format!("http://{}/", listener.local_addr().map_err(|e| e.to_string())?);
    eprintln!("interface : {url}  (Ctrl-C pour arrêter)");
    if open_browser {
        let _ = std::process::Command::new("xdg-open").arg(&url).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
    }
    axum::serve(listener, app).with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    }).await.map_err(|e| format!("serveur : {e}"))
}
