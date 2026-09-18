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
/// `login`, `CLEVER_TOKEN`/`CLEVER_SECRET`, `CLEVER_COMPOSE_API_TOKEN`) pour un
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
    user: Option<String>,
    email: Option<String>,
    /// Propriétaires que ces jetons peuvent viser : l'espace personnel
    /// (`user_…`) et les organisations (`orga_…`), lus à la validation.
    /// `X-CC-Org` doit en faire partie avant tout accès qui ne passe pas
    /// par l'API Clever Cloud (bucket d'assets du serveur).
    owners: Vec<String>,
    at: std::time::Instant,
}

pub struct State {
    /// Identifiants du serveur, pour les clients sans jetons.
    pub fallback: Option<Session>,
    pub locals: local::Locals,
    pub oauth: OauthConsumer,
    /// Base publique du serveur : `--public-url` si donnée ; sinon
    /// déduite de chaque requête (`Host`, `X-Forwarded-Proto`), voir
    /// [`State::public_base_for`].
    pub public_base: Option<String>,
    pub listen: SocketAddr,
    /// Origines autorisées en CORS (interface servie ailleurs) ; aussi les
    /// seules destinations acceptées pour le retour après la danse OAuth.
    pub origins: Vec<String>,
    /// Request tokens en attente d'autorisation → (secret, URL de retour,
    /// date) ; expirés après [`PENDING_TTL`], au plus [`PENDING_MAX`].
    pub pending: std::sync::Mutex<std::collections::HashMap<String, (String, String, std::time::Instant)>>,
    /// Connexions commencées dont le request token n'est pas encore
    /// inséré dans `pending` (en attente de Clever Cloud) : comptées dans
    /// le plafond, sinon des débuts simultanés le dépassent.
    pub pending_reserved: std::sync::atomic::AtomicUsize,
    /// Limitation par adresse des débuts de connexion et des jetons
    /// refusés : ces deux chemins appellent Clever Cloud pour un client
    /// qui n'a rien prouvé.
    pub limiter: Limiter,
    /// Clé exigée des clients (`Authorization: Bearer`), pour un serveur
    /// exposé au-delà du poste.
    pub api_key: Option<String>,
    /// Exécution des tâches depuis la machine du serveur (`run: local`,
    /// `seed --local`) permise : serveur du poste seulement, voir
    /// [`LocalExec`] et [`api::local_execution_allowed`].
    pub local_exec: bool,
    validated: std::sync::Mutex<std::collections::HashMap<String, Validated>>,
    /// Résolveurs d'images partagés (chacun porte son runtime et son
    /// client OCI, coûteux à recréer par requête) : avec les identifiants
    /// Docker du poste, et anonyme pour les projets déposés. Créés à la
    /// première demande.
    registries: std::sync::Mutex<[Option<Arc<crate::plan::registry::Registry>>; 2]>,
    /// Opérations longues en cours (`up`, `down`, `seed`…) et arrêt
    /// demandé : après un Ctrl-C, plus aucune nouvelle opération n'est
    /// acceptée et le serveur attend la fin de celles qui tournent.
    pub running: std::sync::atomic::AtomicUsize,
    pub shutting_down: std::sync::atomic::AtomicBool,
    pub idle: tokio::sync::Notify,
}

impl State {
    /// Réserve une place pour une opération longue ; `None` si le serveur
    /// s'arrête. La garde décrémente et réveille l'arrêt à la fin.
    pub fn begin_operation(self: &Arc<Self>) -> Option<RunningOp> {
        use std::sync::atomic::Ordering;
        if self.shutting_down.load(Ordering::SeqCst) {
            return None;
        }
        self.running.fetch_add(1, Ordering::SeqCst);
        Some(RunningOp(self.clone()))
    }
}

pub struct RunningOp(Arc<State>);

impl Drop for RunningOp {
    fn drop(&mut self) {
        if self.0.running.fetch_sub(1, std::sync::atomic::Ordering::SeqCst) == 1 {
            self.0.idle.notify_waiters();
        }
    }
}

/// L'exécution locale des tâches est-elle possible sur ce serveur ?
/// `Auto` : oui si rien n'indique un serveur publié (écoute locale, sans
/// `--public-url`, `--cors` ni `--api-key`) ; `Off` la refuse toujours ;
/// `On` la permet malgré ces indices (un proxy devant un serveur qui
/// reste personnel). Dans tous les cas, chaque requête doit venir du
/// poste (pair loopback, aucun en-tête de proxy, aucun jeton propre).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LocalExec {
    #[default]
    Auto,
    On,
    Off,
}

impl std::str::FromStr for LocalExec {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "auto" => Ok(LocalExec::Auto),
            "on" => Ok(LocalExec::On),
            "off" => Ok(LocalExec::Off),
            _ => Err(format!("`{s}` : auto, on ou off attendu")),
        }
    }
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
    pub local_exec: LocalExec,
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
/// Un request token non autorisé en dix minutes est oublié.
pub const PENDING_TTL: std::time::Duration = std::time::Duration::from_secs(600);
/// Connexions en attente au plus (au-delà : 429).
pub const PENDING_MAX: usize = 1000;

/// Fenêtre fixe par adresse : au plus `max` événements par `window`.
/// Sert aux débuts de connexion (un request token chez Clever Cloud par
/// appel) et aux jetons refusés (un `/v2/self` par tentative).
pub struct Limiter {
    window: std::time::Duration,
    max: u32,
    hits: std::sync::Mutex<std::collections::HashMap<std::net::IpAddr, (std::time::Instant, u32)>>,
}

impl Limiter {
    pub fn new(window: std::time::Duration, max: u32) -> Limiter {
        Limiter { window, max, hits: Default::default() }
    }

    /// Compte un événement pour `ip` ; `false` si la fenêtre est pleine.
    /// Sans adresse (pas de `ConnectInfo`), rien n'est limité.
    pub fn allow(&self, ip: Option<std::net::IpAddr>) -> bool {
        let Some(ip) = ip else { return true };
        let now = std::time::Instant::now();
        let mut hits = self.hits.lock().unwrap_or_else(|e| e.into_inner());
        if hits.len() > 10_000 {
            hits.retain(|_, (t, _)| now.duration_since(*t) < self.window);
        }
        let e = hits.entry(ip).or_insert((now, 0));
        if now.duration_since(e.0) >= self.window {
            *e = (now, 0);
        }
        e.1 += 1;
        e.1 <= self.max
    }
}

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
    /// Base publique pour cette requête : `--public-url`, sinon le
    /// schéma de `X-Forwarded-Proto` (un proxy devant, comme sur Clever
    /// Cloud) ou `http`, et l'hôte de `Host` (celui que le navigateur a
    /// appelé ; jamais `X-Forwarded-Host`, qu'un client peut poser
    /// lui-même). En écoute locale, `Host` a déjà passé `same_site`.
    pub fn public_base_for(&self, headers: &axum::http::HeaderMap) -> String {
        if let Some(b) = &self.public_base {
            return b.clone();
        }
        let h = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).map(|v| v.split(',').next().unwrap_or(v).trim().to_string()).filter(|v| !v.is_empty());
        let scheme = h("x-forwarded-proto").filter(|s| s == "http" || s == "https").unwrap_or_else(|| "http".into());
        let host = h("host").unwrap_or_else(|| self.listen.to_string());
        format!("{scheme}://{host}")
    }

    /// Le résolveur d'images d'un projet : avec les identifiants Docker
    /// du poste pour un répertoire du poste, anonyme pour un projet
    /// déposé. Partagé entre les requêtes.
    pub fn registry(&self, confine: bool) -> Result<Arc<crate::plan::registry::Registry>, String> {
        let mut slots = self.registries.lock().unwrap_or_else(|e| e.into_inner());
        let slot = &mut slots[usize::from(confine)];
        if let Some(r) = slot {
            return Ok(r.clone());
        }
        let r = Arc::new(if confine { crate::plan::registry::Registry::anonymous()? } else { crate::plan::registry::Registry::new()? });
        *slot = Some(r.clone());
        Ok(r)
    }

    /// `host` (minuscule, `hôte[:port]`) est un nom sous lequel ce serveur
    /// accepte d'être appelé : l'adresse d'écoute, `localhost` et les
    /// adresses de bouclage sur le port d'écoute, l'hôte de `--public-url`,
    /// ceux des origines `--cors`.
    pub fn host_allowed(&self, host: &str) -> bool {
        let port = self.listen.port();
        let of_url = |u: &str| u.split("://").nth(1).map(|h| h.trim_end_matches('/').to_lowercase());
        let mut ok: Vec<String> = vec![self.listen.to_string(), format!("localhost:{port}"), format!("127.0.0.1:{port}"), format!("[::1]:{port}")];
        if port == 80 {
            ok.extend(["localhost".into(), "127.0.0.1".into(), "[::1]".into(), self.listen.ip().to_string()]);
        }
        ok.extend(self.public_base.as_deref().and_then(of_url));
        ok.extend(self.origins.iter().filter_map(|o| of_url(o)));
        ok.iter().any(|h| h == host)
    }

    /// L'API pour un client : ses jetons (validés chez Clever Cloud, cache
    /// de cinq minutes), sinon les identifiants du serveur. `Err` : rien
    /// d'utilisable, ou jetons refusés.
    ///
    /// Les identifiants du serveur ne sont prêtés qu'à une requête **du
    /// poste** (`Client::from_host` : pair loopback, sans en-tête de
    /// proxy) : derrière un reverse proxy vers loopback, chaque client
    /// envoie les siens.
    pub async fn api_for(&self, c: &api::Client) -> Result<(Api, Option<String>), String> {
        let Some(t) = c.tokens.as_ref() else {
            return match self.fallback.as_ref() {
                Some(s) if c.from_host => Ok((s.api.clone(), None)),
                Some(_) => {
                    slog("client sans jetons refusé (requête qui ne vient pas du poste : pair non loopback ou en-tête de proxy)");
                    Err("aucun identifiant : les identifiants du serveur ne servent qu'au poste ; connectez-vous (jetons X-CC-Token/X-CC-Secret ou X-CC-Api-Token)".to_string())
                }
                None => {
                    slog("client sans jetons refusé (le serveur n'a pas d'identifiants)");
                    Err("aucun identifiant : connectez-vous (jetons X-CC-Token/X-CC-Secret ou X-CC-Api-Token)".to_string())
                }
            };
        };
        let key = t.key();
        if let Some(v) = self.validated.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            if v.at.elapsed() < VALIDATION_TTL {
                return Ok((v.api.clone(), v.email.clone()));
            }
        }
        if !self.limiter.allow(c.peer) {
            slog(&format!("jetons non validés : trop de tentatives depuis {}", c.peer.map(|p| p.to_string()).unwrap_or_default()));
            return Err("trop de tentatives de connexion : réessayez dans une minute".to_string());
        }
        let api = Api::new(String::new(), t.credentials(&self.oauth));
        let (user, email) = match api.whoami().await {
            Ok(me) => me,
            Err(e) => {
                slog(&format!("jetons refusés par Clever Cloud : {e}"));
                return Err(format!("jetons refusés par Clever Cloud : {e}"));
            }
        };
        let owners: Vec<String> = match api.organisations().await {
            Ok(orgs) => orgs.into_iter().map(|o| o.id).collect(),
            Err(e) => {
                slog(&format!("organisations du client illisibles : {e}"));
                return Err(format!("organisations du client illisibles : {e}"));
            }
        };
        slog(&format!("jetons validés : {} ({} propriétaire(s), cache {} s)", email.as_deref().unwrap_or("utilisateur sans email"), owners.len(), VALIDATION_TTL.as_secs()));
        let mut cache = self.validated.lock().unwrap_or_else(|e| e.into_inner());
        cache.retain(|_, v| v.at.elapsed() < VALIDATION_TTL);
        cache.insert(key, Validated { api: api.clone(), user, email: email.clone(), owners, at: std::time::Instant::now() });
        Ok((api, email))
    }

    /// Le client à jetons peut-il viser `owner` ? D'après la liste lue à
    /// la validation de ses jetons. Un client sans jetons (identifiants
    /// du serveur, requête du poste) n'est pas restreint ici.
    pub fn may_own(&self, c: &api::Client, owner: &str) -> bool {
        let Some(t) = c.tokens.as_ref() else { return true };
        self.validated.lock().unwrap_or_else(|e| e.into_inner()).get(&t.key()).is_some_and(|v| v.owners.iter().any(|o| o == owner))
    }

    /// Qui dépose ou consulte un projet : l'id Clever Cloud du client à
    /// jetons (validés), `None` pour un client sans jetons servi par les
    /// identifiants du serveur. Erreur si rien n'est utilisable.
    pub async fn owner_of(&self, c: &api::Client) -> Result<Option<String>, String> {
        let Some(t) = c.tokens.as_ref() else {
            return self.api_for(c).await.map(|_| None);
        };
        self.api_for(c).await?;
        Ok(self.validated.lock().unwrap_or_else(|e| e.into_inner()).get(&t.key()).and_then(|v| v.user.clone()))
    }
}

/// Exécution locale permise par le serveur : un indice de publication
/// (écoute non locale, `--public-url`, `--cors`, `--api-key`) la coupe en
/// `Auto` ; `On`/`Off` tranchent.
pub fn local_exec_policy(mode: LocalExec, listen: SocketAddr, public_url: bool, cors: bool, api_key: bool) -> bool {
    match mode {
        LocalExec::On => true,
        LocalExec::Off => false,
        LocalExec::Auto => listen.ip().is_loopback() && !public_url && !cors && !api_key,
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
    let public_base = o.public_url.map(|u| u.trim_end_matches('/').to_string());
    let locals = local::Locals::default();
    if let Some(dir) = o.initial_dir {
        if dir.join("compose.yaml").is_file() || dir.join("compose.yml").is_file() || dir.join("docker-compose.yaml").is_file() || dir.join("docker-compose.yml").is_file() {
            locals.add_dir(dir, false, None)?;
        }
    }
    // Les identifiants du serveur ne se prêtent qu'à des clients du même
    // poste : ailleurs, chacun envoie les siens.
    let fallback = match fallback {
        Some(_) if !listen.ip().is_loopback() => {
            slog(&format!("identifiants du serveur ignorés : écoute sur {listen}, pas locale ; chaque client envoie ses jetons"));
            None
        }
        f => f,
    };
    match &fallback {
        None => slog(&format!("aucun identifiant sur le serveur : chaque client envoie les siens (danse OAuth avec le consumer {}, base URL {})", oauth.key, public_base.clone().unwrap_or_else(|| format!("déduite des requêtes, http://{listen} par défaut")))),
        Some(s) => slog(&format!("identifiants du serveur prêtés aux clients qui n'en envoient pas{}", if s.org.is_empty() { String::new() } else { format!(" (organisation {})", s.org) })),
    }
    slog(&format!("consumer OAuth : {}{}", oauth.key, if std::env::var_os("CLEVER_CONSUMER_KEY").is_some() { " (environnement)" } else { " (embarqué)" }));
    if !origins.is_empty() {
        slog(&format!("CORS : {}", origins.join(", ")));
    }
    slog(&format!("clé d'API : {}", if o.api_key.is_some() { "exigée" } else { "aucune (serveur à réserver au poste)" }));
    let local_exec = local_exec_policy(o.local_exec, listen, public_base.is_some(), !origins.is_empty(), o.api_key.is_some());
    slog(&format!("exécution locale des tâches (run: local, seed --local) : {}", match (local_exec, o.local_exec) {
        (true, LocalExec::On) => "permise (--local-exec on), pour les requêtes du poste seulement",
        (true, _) => "permise, pour les requêtes du poste seulement (--local-exec off pour la couper)",
        (false, LocalExec::Off) => "refusée (--local-exec off)",
        (false, _) => "refusée (serveur publié : écoute non locale, --public-url, --cors ou --api-key ; --local-exec on pour forcer)",
    }));
    if let Some(b) = std::env::var_os(crate::exec::assets::ENV_BUCKET) {
        slog(&format!("assets Cellar : bucket {}", b.to_string_lossy()));
    }
    if !listen.ip().is_loopback() && public_base.is_none() {
        slog("sans --public-url : le callback OAuth est bâti sur l'en-tête Host de chaque requête (X-Forwarded-Proto pour le schéma) ; posez --public-url derrière un proxy pour figer la base");
    }
    let state = Arc::new(State { fallback, locals, oauth, public_base, listen, origins, pending: Default::default(), pending_reserved: Default::default(), limiter: Limiter::new(std::time::Duration::from_secs(60), 20), api_key: o.api_key.clone(), local_exec, validated: Default::default(), registries: Default::default(), running: Default::default(), shutting_down: Default::default(), idle: tokio::sync::Notify::new() });
    let app = api::router(state.clone());
    #[cfg(feature = "ui")]
    let app = if o.ui { app.fallback(assets::serve) } else { app.fallback(api_only) };
    #[cfg(not(feature = "ui"))]
    let app = app.fallback(api_only);
    let app = app
        .layer(axum::middleware::from_fn_with_state(state.clone(), api::require_key))
        .layer(axum::middleware::from_fn_with_state(state.clone(), api::same_site))
        .layer(axum::middleware::from_fn_with_state(state.clone(), api::cors))
        .layer(axum::middleware::from_fn(api::log_request));
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
    // L'adresse du pair est exposée aux routes (`ConnectInfo`) : l'exécution
    // locale exige une connexion depuis le poste.
    let app = app.into_make_service_with_connect_info::<SocketAddr>();
    // Arrêt en deux temps : le premier Ctrl-C refuse les nouvelles
    // opérations et attend celles en cours (un `up` interrompu à mi-chemin
    // laisse la plateforme dans un état que le prochain `up` reprend, mais
    // autant le finir) ; un second Ctrl-C force la sortie.
    let st = state.clone();
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            st.shutting_down.store(true, std::sync::atomic::Ordering::SeqCst);
            loop {
                // Inscrit avant de lire le compteur : un réveil entre les
                // deux n'est pas perdu.
                let notified = st.idle.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let n = st.running.load(std::sync::atomic::Ordering::SeqCst);
                if n == 0 {
                    break;
                }
                slog(&format!("arrêt : {n} opération(s) en cours, attente de leur fin (Ctrl-C à nouveau pour forcer)"));
                tokio::select! {
                    _ = &mut notified => {}
                    _ = tokio::signal::ctrl_c() => {
                        slog("arrêt forcé");
                        std::process::exit(130);
                    }
                }
            }
            slog("arrêt");
        })
        .await
        .map_err(|e| format!("serveur : {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_exec_auto_follows_publication_hints() {
        let local: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let public: SocketAddr = "0.0.0.0:8080".parse().unwrap();
        assert!(local_exec_policy(LocalExec::Auto, local, false, false, false));
        assert!(!local_exec_policy(LocalExec::Auto, public, false, false, false));
        // Loopback derrière un proxy déclaré, ou avec une clé : publié.
        assert!(!local_exec_policy(LocalExec::Auto, local, true, false, false));
        assert!(!local_exec_policy(LocalExec::Auto, local, false, true, false));
        assert!(!local_exec_policy(LocalExec::Auto, local, false, false, true));
        assert!(local_exec_policy(LocalExec::On, public, true, true, true));
        assert!(!local_exec_policy(LocalExec::Off, local, false, false, false));
    }

    #[test]
    fn limiter_counts_per_address_in_a_window() {
        let l = Limiter::new(std::time::Duration::from_secs(60), 3);
        let a: std::net::IpAddr = "203.0.113.7".parse().unwrap();
        let b: std::net::IpAddr = "203.0.113.8".parse().unwrap();
        assert!(l.allow(Some(a)) && l.allow(Some(a)) && l.allow(Some(a)));
        assert!(!l.allow(Some(a)));
        assert!(l.allow(Some(b)));
        assert!(l.allow(None) && l.allow(None) && l.allow(None) && l.allow(None));
    }
}
