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
    /// Base publique du serveur (`http://<adresse d'écoute>` ou
    /// `--public-url`), base du `oauth_callback`.
    pub public_base: String,
    /// Origines autorisées en CORS (interface servie ailleurs) ; aussi les
    /// seules destinations acceptées pour le retour après la danse OAuth.
    pub origins: Vec<String>,
    /// Request tokens en attente d'autorisation → (secret, URL de retour).
    pub pending: std::sync::Mutex<std::collections::HashMap<String, (String, String)>>,
}

/// Ce que le serveur sert et d'où l'interface peut l'appeler.
pub struct ServeOptions {
    pub listen: SocketAddr,
    /// Sert aussi l'interface embarquée (`ui`) ; sinon l'API seule (`api`).
    pub ui: bool,
    /// Origines (`https://ui.exemple`) autorisées en CORS pour une interface
    /// déployée à part.
    pub origins: Vec<String>,
    /// Base publique si le serveur est derrière un proxy (callback OAuth).
    pub public_url: Option<String>,
    pub initial_dir: Option<std::path::PathBuf>,
    pub open_browser: bool,
}

impl State {
    /// L'API de la session, ou une erreur si aucun identifiant n'est posé.
    pub fn api(&self) -> Result<Api, String> {
        self.session.read().unwrap().as_ref().map(|s| s.api.clone()).ok_or_else(|| "aucun identifiant : renseignez un jeton dans l'interface (ou CLEVER_TOKEN/CLEVER_SECRET, CC_API_TOKEN au lancement)".to_string())
    }
}

/// Lance le serveur et bloque jusqu'à Ctrl-C. `session` peut être absente :
/// l'interface demandera les identifiants.
pub async fn serve(o: ServeOptions, session: Option<Session>, oauth: Option<OauthConsumer>) -> Result<(), String> {
    if session.is_none() && oauth.is_none() {
        return Err("aucun identifiant : posez CLEVER_TOKEN et CLEVER_SECRET (ou CC_API_TOKEN), ou un consumer OAuth (CLEVER_CONSUMER_KEY et CLEVER_CONSUMER_SECRET, base URL = adresse publique du serveur) pour vous connecter depuis l'interface".into());
    }
    let listen = o.listen;
    let origins: Vec<String> = o.origins.iter().map(|s| s.trim_end_matches('/').to_string()).collect();
    if let Some(bad) = origins.iter().find(|s| !(s.starts_with("http://") || s.starts_with("https://")) || s[s.find("://").unwrap() + 3..].contains('/')) {
        return Err(format!("--cors `{bad}` : une origine est `http(s)://hôte[:port]`, sans chemin"));
    }
    let public_base = o.public_url.map(|u| u.trim_end_matches('/').to_string()).unwrap_or_else(|| format!("http://{listen}"));
    let locals = local::Locals::default();
    if let Some(dir) = o.initial_dir {
        if dir.join("compose.yaml").is_file() || dir.join("compose.yml").is_file() || dir.join("docker-compose.yaml").is_file() || dir.join("docker-compose.yml").is_file() {
            locals.add_dir(dir, false)?;
        }
    }
    let state = Arc::new(State { session: RwLock::new(session), locals, oauth, public_base, origins, pending: Default::default() });
    let app = api::router(state.clone());
    let app = if o.ui { app.fallback(assets::serve) } else { app.fallback(|| async { (axum::http::StatusCode::NOT_FOUND, "API seule : l'interface est servie ailleurs (clever-compose ui pour l'embarquer)") }) };
    let app = app.layer(axum::middleware::from_fn_with_state(state.clone(), api::cors));
    let listener = tokio::net::TcpListener::bind(listen).await.map_err(|e| format!("écoute sur {listen} : {e}"))?;
    let url = format!("http://{}/", listener.local_addr().map_err(|e| e.to_string())?);
    if o.ui {
        eprintln!("interface : {url}  (Ctrl-C pour arrêter)");
    } else {
        eprintln!("API : {url}api/  (Ctrl-C pour arrêter){}", if state.origins.is_empty() { "\n  aucune origine CORS : passez --cors https://<hôte de l'interface> pour une interface déployée à part" } else { "" });
    }
    if o.open_browser && o.ui {
        let _ = std::process::Command::new("xdg-open").arg(&url).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
    }
    axum::serve(listener, app).with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    }).await.map_err(|e| format!("serveur : {e}"))
}
