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
#[cfg(feature = "ui")]
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

/// Consumer OAuth 1.0 pour la connexion depuis l'interface. Soit celui de
/// l'utilisateur (`CLEVER_CONSUMER_KEY`/`CLEVER_CONSUMER_SECRET`, créé
/// dans la console avec la base URL du serveur : danse classique avec
/// callback), soit, à défaut, celui de clever-tools (constantes publiques
/// du SDK) : ce consumer refuse tout callback (« OAuth callback is
/// invalid ») et se connecte par la page `cli-oauth` de la console, dont
/// le serveur récupère les jetons sur `/v2/self/cli_tokens`.
#[derive(Clone)]
pub struct OauthConsumer {
    pub key: String,
    pub secret: String,
    /// Consumer de clever-tools : connexion par la console.
    pub builtin: bool,
}

impl OauthConsumer {
    /// Le consumer de l'environnement, sinon celui de clever-tools.
    pub fn from_env() -> Self {
        match (std::env::var("CLEVER_CONSUMER_KEY"), std::env::var("CLEVER_CONSUMER_SECRET")) {
            (Ok(key), Ok(secret)) if !key.is_empty() && !secret.is_empty() => OauthConsumer { key, secret, builtin: false },
            _ => OauthConsumer { key: clevercloud_sdk::DEFAULT_CONSUMER_KEY.into(), secret: clevercloud_sdk::DEFAULT_CONSUMER_SECRET.into(), builtin: true },
        }
    }
}

pub struct State {
    pub session: RwLock<Option<Session>>,
    pub locals: local::Locals,
    pub oauth: OauthConsumer,
    /// Base publique du serveur (`http://<adresse d'écoute>` ou
    /// `--public-url`), base du `oauth_callback`.
    pub public_base: String,
    /// Origines autorisées en CORS (interface servie ailleurs) ; aussi les
    /// seules destinations acceptées pour le retour après la danse OAuth.
    pub origins: Vec<String>,
    /// Request tokens en attente d'autorisation → (secret, URL de retour).
    pub pending: std::sync::Mutex<std::collections::HashMap<String, (String, String)>>,
    /// Clé exigée des clients (`Authorization: Bearer`), pour un serveur
    /// exposé au-delà du poste.
    pub api_key: Option<String>,
    /// Connexion par la console en cours (`cli_token` attendu), ou son
    /// dernier échec.
    pub console_login: std::sync::Mutex<ConsoleLogin>,
}

#[derive(Default)]
pub struct ConsoleLogin {
    pub pending: bool,
    pub error: Option<String>,
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
    /// Clé d'API exigée des clients ; `None` : aucune vérification.
    pub api_key: Option<String>,
    pub initial_dir: Option<std::path::PathBuf>,
    pub open_browser: bool,
}

impl State {
    /// L'API de la session, ou une erreur si aucun identifiant n'est posé.
    pub fn api(&self) -> Result<Api, String> {
        self.session.read().unwrap().as_ref().map(|s| s.api.clone()).ok_or_else(|| "aucun identifiant : renseignez un jeton dans l'interface (ou CLEVER_TOKEN/CLEVER_SECRET, CC_API_TOKEN au lancement)".to_string())
    }
}

async fn api_only() -> (axum::http::StatusCode, &'static str) {
    (axum::http::StatusCode::NOT_FOUND, "API seule : l'interface est servie ailleurs (clever-compose ui, binaire avec la feature `ui`, pour l'embarquer)")
}

/// Lance le serveur et bloque jusqu'à Ctrl-C. `session` peut être absente :
/// l'interface demandera les identifiants.
pub async fn serve(o: ServeOptions, session: Option<Session>, oauth: OauthConsumer) -> Result<(), String> {
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
    if session.is_none() {
        eprintln!("aucun identifiant dans l'environnement : l'interface demandera la connexion ({})", if oauth.builtin { "par la console Clever Cloud, consumer de clever-tools" } else { "danse OAuth avec le consumer CLEVER_CONSUMER_KEY" });
    }
    let state = Arc::new(State { session: RwLock::new(session), locals, oauth, public_base, origins, pending: Default::default(), api_key: o.api_key.clone(), console_login: Default::default() });
    let app = api::router(state.clone());
    #[cfg(feature = "ui")]
    let app = if o.ui { app.fallback(assets::serve) } else { app.fallback(api_only) };
    #[cfg(not(feature = "ui"))]
    let app = app.fallback(api_only);
    let app = app.layer(axum::middleware::from_fn_with_state(state.clone(), api::require_key)).layer(axum::middleware::from_fn_with_state(state.clone(), api::cors));
    let listener = tokio::net::TcpListener::bind(listen).await.map_err(|e| format!("écoute sur {listen} : {e}"))?;
    let url = format!("http://{}/", listener.local_addr().map_err(|e| e.to_string())?);
    if o.ui {
        eprintln!("interface : {url}  (Ctrl-C pour arrêter)");
    } else {
        eprintln!("API : {url}api/  (Ctrl-C pour arrêter){}", if state.origins.is_empty() { "\n  (sans --cors : interface attendue sur la même origine, par ex. derrière le proxy Vite ; --cors https://<hôte> pour une interface servie ailleurs)" } else { "" });
    }
    if o.open_browser && o.ui {
        let _ = std::process::Command::new("xdg-open").arg(&url).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
    }
    axum::serve(listener, app).with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    }).await.map_err(|e| format!("serveur : {e}"))
}
