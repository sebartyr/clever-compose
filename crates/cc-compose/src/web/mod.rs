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
use std::sync::Arc;

use crate::exec::platform::Api;

pub use crate::auth::Consumer as OauthConsumer;

/// Un client authentifié : les jetons envoyés dans la requête
/// (`X-CC-Token` + `X-CC-Secret`, ou `X-CC-Api-Token`), validés chez
/// Clever Cloud, ou les identifiants du serveur (`credentials.json` du
/// `login`, `CLEVER_TOKEN`/`CLEVER_SECRET`, `CC_API_TOKEN`) pour un
/// client qui n'en envoie pas : l'usage local, `clever-compose ui` sur
/// son poste.
#[derive(Clone)]
pub struct Session {
    pub api: Api,
    pub org: String,
}

/// Jetons validés récemment : on n'interroge pas `/v2/self` à chaque
/// requête. Clé : les jetons tels qu'envoyés.
struct Validated {
    api: Api,
    email: Option<String>,
    at: std::time::Instant,
}

pub struct State {
    /// Identifiants du serveur, pour les clients sans jetons.
    pub fallback: Option<Session>,
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
    validated: std::sync::Mutex<std::collections::HashMap<String, Validated>>,
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

/// Jetons envoyés par un client.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum ClientTokens {
    /// Jetons issus de la danse de ce serveur, signés avec son consumer.
    OAuth1 { token: String, secret: String },
    Bearer(String),
}

impl ClientTokens {
    fn key(&self) -> String {
        match self {
            ClientTokens::OAuth1 { token, secret } => format!("o:{token}:{secret}"),
            ClientTokens::Bearer(t) => format!("b:{t}"),
        }
    }
    fn credentials(&self, server: &OauthConsumer) -> clevercloud_sdk::Credentials {
        match self {
            ClientTokens::OAuth1 { token, secret } => clevercloud_sdk::Credentials::OAuth1 { token: token.clone(), secret: secret.clone(), consumer_key: server.key.clone(), consumer_secret: server.secret.clone() },
            ClientTokens::Bearer(t) => clevercloud_sdk::Credentials::Bearer { token: t.clone() },
        }
    }
}

const VALIDATION_TTL: std::time::Duration = std::time::Duration::from_secs(300);

/// Journal du serveur sur stderr, horodaté : requêtes, clients validés
/// ou refusés, connexions, opérations et leur issue. Toujours verbeux
/// (demande du propriétaire, 17/09/2026) : ce qu'un serveur fait avec
/// des jetons d'autrui doit se lire.
pub fn slog(line: &str) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let hms = crate::exec::inspect::rfc3339(now);
    let hms = hms.get(11..19).unwrap_or(&hms);
    eprintln!("{hms}  {line}");
}

impl State {
    /// L'API pour un client : ses jetons (validés chez Clever Cloud, cache
    /// de cinq minutes), sinon les identifiants du serveur. `Err` : rien
    /// d'utilisable, ou jetons refusés.
    pub async fn api_for(&self, tokens: Option<&ClientTokens>) -> Result<(Api, Option<String>), String> {
        let Some(t) = tokens else {
            return match self.fallback.as_ref() {
                Some(s) => Ok((s.api.clone(), None)),
                None => {
                    slog("client sans jetons refusé (le serveur n'a pas d'identifiants)");
                    Err("aucun identifiant : connectez-vous (jetons X-CC-Token/X-CC-Secret ou X-CC-Api-Token)".to_string())
                }
            };
        };
        let key = t.key();
        if let Some(v) = self.validated.lock().unwrap().get(&key) {
            if v.at.elapsed() < VALIDATION_TTL {
                return Ok((v.api.clone(), v.email.clone()));
            }
        }
        let api = Api::new(String::new(), t.credentials(&self.oauth));
        let (_, email) = match api.whoami().await {
            Ok(me) => me,
            Err(e) => {
                slog(&format!("jetons refusés par Clever Cloud : {e}"));
                return Err(format!("jetons refusés par Clever Cloud : {e}"));
            }
        };
        slog(&format!("jetons validés : {} (cache {} s)", email.as_deref().unwrap_or("utilisateur sans email"), VALIDATION_TTL.as_secs()));
        let mut cache = self.validated.lock().unwrap();
        cache.retain(|_, v| v.at.elapsed() < VALIDATION_TTL);
        cache.insert(key, Validated { api: api.clone(), email: email.clone(), at: std::time::Instant::now() });
        Ok((api, email))
    }
}

async fn api_only() -> (axum::http::StatusCode, &'static str) {
    (axum::http::StatusCode::NOT_FOUND, "API seule : l'interface est servie ailleurs (clever-compose ui, binaire avec la feature `ui`, pour l'embarquer)")
}

/// Lance le serveur et bloque jusqu'à Ctrl-C. `session` peut être absente :
/// l'interface demandera les identifiants.
pub async fn serve(o: ServeOptions, fallback: Option<Session>, oauth: OauthConsumer) -> Result<(), String> {
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
    match &fallback {
        None => slog(&format!("aucun identifiant sur le serveur : chaque client envoie les siens (danse OAuth avec le consumer {}, base URL {public_base})", oauth.key)),
        Some(s) => slog(&format!("identifiants du serveur prêtés aux clients qui n'en envoient pas{}", if s.org.is_empty() { String::new() } else { format!(" (organisation {})", s.org) })),
    }
    slog(&format!("consumer OAuth : {}{}", oauth.key, if std::env::var_os("CLEVER_CONSUMER_KEY").is_some() { " (environnement)" } else { " (embarqué)" }));
    if !origins.is_empty() {
        slog(&format!("CORS : {}", origins.join(", ")));
    }
    slog(&format!("clé d'API : {}", if o.api_key.is_some() { "exigée" } else { "aucune (serveur à réserver au poste)" }));
    if let Some(b) = std::env::var_os(crate::exec::assets::ENV_BUCKET) {
        slog(&format!("assets Cellar : bucket {}", b.to_string_lossy()));
    }
    let state = Arc::new(State { fallback, locals, oauth, public_base, origins, pending: Default::default(), api_key: o.api_key.clone(), validated: Default::default() });
    let app = api::router(state.clone());
    #[cfg(feature = "ui")]
    let app = if o.ui { app.fallback(assets::serve) } else { app.fallback(api_only) };
    #[cfg(not(feature = "ui"))]
    let app = app.fallback(api_only);
    let app = app.layer(axum::middleware::from_fn_with_state(state.clone(), api::require_key)).layer(axum::middleware::from_fn_with_state(state.clone(), api::cors)).layer(axum::middleware::from_fn(api::log_request));
    let listener = tokio::net::TcpListener::bind(listen).await.map_err(|e| format!("écoute sur {listen} : {e}"))?;
    let url = format!("http://{}/", listener.local_addr().map_err(|e| e.to_string())?);
    if o.ui {
        slog(&format!("interface : {url}  (Ctrl-C pour arrêter)"));
    } else {
        slog(&format!("API : {url}api/  (Ctrl-C pour arrêter){}", if state.origins.is_empty() { " — sans --cors : interface attendue sur la même origine, par ex. derrière le proxy Vite" } else { "" }));
    }
    if o.open_browser && o.ui {
        let _ = std::process::Command::new("xdg-open").arg(&url).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
    }
    axum::serve(listener, app).with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    }).await.map_err(|e| format!("serveur : {e}"))
}
