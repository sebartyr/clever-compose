//! Routes de l'API locale. Tout passe par les mêmes fonctions que la CLI :
//! `plan::cli::run`, `exec::reconcile::{up, down}`, `exec::inspect`,
//! `emit::export`. Les opérations longues renvoient un flux SSE d'événements
//! `log` (une ligne du journal), puis `done` (`ok` ou `error: …`).

use std::convert::Infallible;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Multipart, Path as UrlPath, Query, State as AxState};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use cc_compose_model::plan::Plan;
use serde::Deserialize;
use serde_json::{json, Value as J};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tokio_stream::{Stream, StreamExt};

use super::State;
use crate::diagnostic::{Diagnostics, Severity};
use crate::exec::platform::Platform;
use crate::exec::reconcile::Log;

type Shared = Arc<State>;

/// CORS pour une interface servie sur une autre origine (`--cors`) : la
/// pré-requête `OPTIONS` reçoit les en-têtes sans passer par les routes, les
/// réponses des origines listées les reçoivent aussi. Rien n'est ajouté
/// pour une origine inconnue (le navigateur refuse alors la réponse) ni en
/// même origine (aucun en-tête `Origin` à comparer, ou identique).
pub async fn cors(AxState(s): AxState<Shared>, req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    use axum::http::{header, HeaderValue, Method};
    let origin = req.headers().get(header::ORIGIN).and_then(|v| v.to_str().ok()).map(|o| o.trim_end_matches('/').to_string());
    let allowed = origin.as_deref().filter(|o| s.origins.iter().any(|a| a == o)).map(|o| o.to_string());
    if req.method() == Method::OPTIONS {
        let mut r = StatusCode::NO_CONTENT.into_response();
        if let Some(o) = &allowed {
            let h = r.headers_mut();
            h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_str(o).unwrap());
            h.insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("GET, POST, PUT, DELETE, OPTIONS"));
            h.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("Content-Type, Authorization, X-CC-Token, X-CC-Secret, X-CC-Api-Token, X-CC-Org"));
            h.insert(header::ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
        }
        return r;
    }
    let mut r = next.run(req).await;
    if let Some(o) = &allowed {
        r.headers_mut().insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_str(o).unwrap());
        r.headers_mut().insert(header::VARY, HeaderValue::from_static("Origin"));
    }
    r
}

/// Clé d'API (`--api-key`) : exigée en `Authorization: Bearer` sur tout
/// `/api/*` sauf `/api/info` (qui dit qu'une clé est requise) et le
/// callback OAuth (appelé par Clever Cloud). Sans clé configurée, rien
/// n'est vérifié : le serveur est local.
pub async fn require_key(AxState(s): AxState<Shared>, req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    if let Some(key) = &s.api_key {
        let path = req.uri().path();
        let exempt = !path.starts_with("/api/") || path == "/api/info" || path == "/api/session/oauth/callback" || req.method() == axum::http::Method::OPTIONS;
        if !exempt {
            let given = req.headers().get(axum::http::header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")).map(str::trim);
            if given != Some(key.as_str()) {
                return ApiError(StatusCode::UNAUTHORIZED, "clé d'API absente ou invalide (--api-key du serveur)".into()).into_response();
            }
        }
    }
    next.run(req).await
}

/// Contre le CSRF et le DNS rebinding sur `/api/*` :
///
/// - `Host` : en écoute locale, seuls l'adresse d'écoute, `localhost`,
///   les hôtes de `--public-url` et de `--cors` sont admis (un site
///   qui fait pointer son nom vers 127.0.0.1 est refusé, 421). En écoute
///   non locale sans `--public-url`, l'hôte n'est pas connu du serveur
///   (application Clever Cloud) : pas de vérification.
/// - Requêtes mutantes (`POST`, `PUT`, `DELETE`) : un `Origin` présent doit
///   être la même origine que `Host` ou une origine `--cors`, et
///   `Sec-Fetch-Site: cross-site` est refusé (403). Les navigateurs posent
///   `Origin` sur toute requête cross-origin, `fetch` en `no-cors` et
///   formulaires compris ; les clients hors navigateur n'en ont pas.
///   Le callback OAuth (`GET`, redirigé par Clever Cloud) n'est pas concerné.
pub async fn same_site(AxState(s): AxState<Shared>, req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    use axum::http::{header, Method};
    let path = req.uri().path().to_string();
    let method = req.method().clone();
    if !path.starts_with("/api/") || method == Method::OPTIONS {
        return next.run(req).await;
    }
    // Lu d'un bloc : un emprunt de `req` (corps non `Sync`) gardé jusqu'à
    // l'`await` rendrait le futur non `Send`.
    let (host, fetch_site, origin) = {
        let get = |name: &str| req.headers().get(name).and_then(|v| v.to_str().ok()).map(|v| v.trim().to_string());
        (get("host").map(|h| h.to_lowercase()).unwrap_or_default(), get("sec-fetch-site").map(|v| v.to_lowercase()), get(header::ORIGIN.as_str()).map(|o| o.trim_end_matches('/').to_string()))
    };
    if (s.listen.ip().is_loopback() || s.public_base.is_some()) && !host.is_empty() && !s.host_allowed(&host) {
        super::slog(&format!("refusé : Host `{host}` inconnu (écoute {}, --public-url, --cors)", s.listen));
        return ApiError(StatusCode::MISDIRECTED_REQUEST, format!("Host `{host}` : ce serveur ne répond que pour {}", s.listen)).into_response();
    }
    if [Method::POST, Method::PUT, Method::DELETE, Method::PATCH].contains(&method) {
        if fetch_site.as_deref() == Some("cross-site") {
            super::slog(&format!("refusé : {method} {path} cross-site (Sec-Fetch-Site)"));
            return ApiError(StatusCode::FORBIDDEN, "requête cross-site refusée".into()).into_response();
        }
        if let Some(origin) = origin {
            let same = origin.split("://").nth(1).is_some_and(|o| o.eq_ignore_ascii_case(&host));
            if !same && !s.origins.contains(&origin) {
                super::slog(&format!("refusé : {method} {path} depuis l'origine `{origin}` (Host {host})"));
                return ApiError(StatusCode::FORBIDDEN, format!("origine `{origin}` refusée : ni ce serveur, ni une origine --cors")).into_response();
            }
        }
    }
    next.run(req).await
}

/// Une ligne par requête : méthode, chemin, statut, durée, organisation
/// visée, jetons propres ou non. Les fichiers de l'interface sont tus.
pub async fn log_request(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let query = req.uri().query().map(|q| format!("?{q}")).unwrap_or_default();
    let org = req.headers().get("x-cc-org").and_then(|v| v.to_str().ok()).map(str::to_string);
    let own = req.headers().contains_key("x-cc-token") || req.headers().contains_key("x-cc-api-token");
    let started = std::time::Instant::now();
    let resp = next.run(req).await;
    if path.starts_with("/api/") && method != axum::http::Method::OPTIONS {
        let ms = started.elapsed().as_millis();
        let who = match (own, org) {
            (true, Some(o)) => format!("  [jetons du client, {o}]"),
            (true, None) => "  [jetons du client]".to_string(),
            (false, Some(o)) => format!("  [identifiants du serveur, {o}]"),
            (false, None) => String::new(),
        };
        super::slog(&format!("{method} {path}{query} → {} ({ms} ms){who}", resp.status().as_u16()));
    }
    resp
}

pub fn router(state: Shared) -> Router {
    Router::new()
        .route("/api/info", get(info))
        .route("/api/session", get(session_get))
        .route("/api/session/login", post(login_start))
        .route("/api/session/oauth/callback", get(oauth_callback))
        .route("/api/orgs", get(orgs))
        .route("/api/platform/projects", get(platform_projects))
        .route("/api/platform/projects/{project}/inventory", get(inventory))
        .route("/api/platform/projects/{project}/logs", get(logs))
        .route("/api/platform/projects/{project}/ps", post(ps))
        .route("/api/platform/projects/{project}/down", post(down))
        .route("/api/platform/projects/{project}/stop", post(stop))
        .route("/api/platform/projects/{project}/start", post(start))
        .route("/api/platform/projects/{project}/assets", get(assets_get))
        .route("/api/platform/projects/{project}/assets/open", post(assets_open))
        .route("/api/local", get(local_list).post(local_add_dir))
        .route("/api/local/upload", post(local_upload))
        .route("/api/local/{id}", axum::routing::delete(local_remove))
        .route("/api/local/{id}/files", get(local_files))
        .route("/api/local/{id}/file", get(local_file_get).put(local_file_put))
        .route("/api/local/{id}/plan", get(local_plan))
        .route("/api/local/{id}/up", post(local_up))
        .route("/api/local/{id}/seed", post(local_seed))
        .route("/api/local/{id}/seed/upload", post(local_seed_upload))
        .route("/api/local/{id}/export", post(local_export))
        .route("/api/local/{id}/scaling", axum::routing::put(local_scaling))
        .route("/api/local/{id}/assets/sync", post(local_assets_sync))
        .route("/api/flavors", get(flavors))
        .layer(axum::extract::DefaultBodyLimit::max(256 * 1024 * 1024))
        .with_state(state)
}

struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, msg.into())
}

fn internal(msg: impl Into<String>) -> ApiError {
    ApiError(StatusCode::INTERNAL_SERVER_ERROR, msg.into())
}

async fn info(AxState(s): AxState<Shared>) -> Json<J> {
    let stamp = env!("CLEVER_COMPOSE_BUILD_STAMP");
    let (commit, secs) = stamp.split_once(' ').unwrap_or((stamp, "0"));
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"), "commit": commit, "built": crate::exec::inspect::rfc3339(secs.parse().unwrap_or(0)),
        "bundled": cfg!(feature = "ui-bundled"), "assets_bucket": std::env::var(crate::exec::assets::ENV_BUCKET).ok(), "api_key": s.api_key.is_some(),
        // Le serveur a-t-il ses propres identifiants (clients sans jetons servis) ?
        "server_credentials": s.fallback.is_some(),
        // Exécution depuis le poste (`seed --local`, `init: local`) permise
        // par le serveur ; chaque requête doit en plus venir du poste sans
        // jetons propres (`local_execution_allowed`).
        "local_exec": s.local_exec,
    }))
}

/// Jetons et organisation de la requête. `X-CC-Token` + `X-CC-Secret`
/// (OAuth 1.0, issus de la danse de ce serveur) ou `X-CC-Api-Token`
/// (bearer) ; `X-CC-Org` pour l'organisation visée.
#[derive(Clone, Default)]
pub struct Client {
    pub tokens: Option<super::ClientTokens>,
    pub org: Option<String>,
    /// La requête vient du poste : pair loopback et aucun en-tête de proxy
    /// (voir [`from_host`]). Seule une telle requête reçoit les
    /// identifiants du serveur ou l'exécution locale.
    pub from_host: bool,
}

/// En-têtes qu'un proxy pose : leur présence dit que la connexion
/// loopback ne vient pas du poste.
const PROXY_HEADERS: &[&str] = &["forwarded", "x-forwarded-for", "x-forwarded-host", "x-forwarded-proto", "x-real-ip", "via"];

/// Requête du poste : pair loopback (`ConnectInfo`, posé par `serve`),
/// sans en-tête de proxy.
pub fn from_host(headers: &axum::http::HeaderMap, peer: Option<std::net::SocketAddr>) -> bool {
    peer.is_some_and(|p| p.ip().is_loopback()) && !PROXY_HEADERS.iter().any(|h| headers.contains_key(*h))
}

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for Client {
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(parts: &mut axum::http::request::Parts, _s: &S) -> Result<Self, Self::Rejection> {
        let h = |name: &str| parts.headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
        let tokens = match (h("x-cc-token"), h("x-cc-secret"), h("x-cc-api-token")) {
            (Some(token), Some(secret), _) => Some(super::ClientTokens::OAuth1 { token, secret }),
            (_, _, Some(t)) => Some(super::ClientTokens::Bearer(t)),
            _ => None,
        };
        let peer = parts.extensions.get::<axum::extract::ConnectInfo<std::net::SocketAddr>>().map(|c| c.0);
        Ok(Client { tokens, org: h("x-cc-org"), from_host: from_host(&parts.headers, peer) })
    }
}

/// Qui est le client : jetons valides ?, utilisateur, organisation demandée.
/// `authenticated: false` avec `reason` quand les jetons sont refusés :
/// l'interface propose alors la connexion.
async fn session_get(AxState(s): AxState<Shared>, c: Client) -> Json<J> {
    match s.api_for(&c).await {
        Ok((_, email)) => Json(json!({ "authenticated": true, "email": email, "org": c.org.unwrap_or_default(), "own_tokens": c.tokens.is_some() })),
        Err(reason) => Json(json!({ "authenticated": false, "reason": reason, "org": c.org.unwrap_or_default(), "own_tokens": c.tokens.is_some() })),
    }
}

use crate::auth::oauth_enc;

#[derive(Deserialize)]
struct OauthStart {
    /// Où renvoyer l'utilisateur à la fin (mode `redirect`) : l'interface
    /// (même origine, ou une origine `--cors`).
    #[serde(rename = "return")]
    return_to: Option<String>,
}

/// Début de la connexion d'un navigateur : request token avec callback
/// sur ce serveur, URL d'autorisation à ouvrir. Le callback renvoie les
/// jetons dans le fragment de l'URL de retour.
async fn login_start(AxState(s): AxState<Shared>, headers: axum::http::HeaderMap, body: Option<Json<OauthStart>>) -> Result<Json<J>, ApiError> {
    let q = body.map(|Json(b)| b).unwrap_or(OauthStart { return_to: None });
    let public_base = s.public_base_for(&headers);
    let return_to = match q.return_to.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
        None => "/".to_string(),
        Some(r) if r.starts_with('/') && !r.starts_with("//") => r.to_string(),
        Some(r) if s.origins.iter().chain(std::iter::once(&public_base)).any(|o| r == o || r.starts_with(&format!("{o}/"))) => r.to_string(),
        Some(r) => return Err(bad(format!("`return={r}` : ni un chemin de ce serveur, ni {public_base} (--public-url), ni une origine passée en --cors"))),
    };
    let callback = format!("{public_base}/api/session/oauth/callback");
    super::slog(&format!("connexion : callback {callback}"));
    let (token, secret, url) = crate::auth::request_token(&s.oauth, &callback).await.map_err(internal)?;
    super::slog(&format!("connexion : request token obtenu, retour prévu vers {return_to}"));
    s.pending.lock().unwrap().insert(token.clone(), (secret, return_to));
    Ok(Json(json!({ "mode": "redirect", "url": url })))
}

#[derive(Deserialize)]
struct OauthCallback {
    oauth_token: String,
    oauth_verifier: String,
}

/// Étape 3 : access token, vérifié, puis retour à l'interface avec les
/// jetons dans le fragment (`#token=…&secret=…`, jamais envoyé au serveur
/// par le navigateur).
async fn oauth_callback(AxState(s): AxState<Shared>, Query(q): Query<OauthCallback>) -> Result<Response, ApiError> {
    let (request_secret, return_to) = s.pending.lock().unwrap().remove(&q.oauth_token).ok_or_else(|| bad("request token inconnu ou déjà utilisé : recommencez la connexion"))?;
    let (token, secret) = crate::auth::access_token(&s.oauth, &q.oauth_token, &request_secret, &q.oauth_verifier).await.map_err(internal)?;
    let mut stored = crate::auth::Stored { token: token.clone(), secret: secret.clone(), consumer_key: None, user_id: None, email: None };
    crate::auth::verify(&mut stored, &s.oauth).await.map_err(|e| ApiError(StatusCode::UNAUTHORIZED, e))?;
    super::slog(&format!("connexion : jetons obtenus pour {}", stored.email.as_deref().unwrap_or("utilisateur sans email")));
    Ok(axum::response::Redirect::temporary(&format!("{return_to}#token={}&secret={}", oauth_enc(&token), oauth_enc(&secret))).into_response())
}

async fn orgs(AxState(s): AxState<Shared>, c: Client) -> Result<Json<J>, ApiError> {
    let (api, _) = s.api_for(&c).await.map_err(|e| ApiError(StatusCode::UNAUTHORIZED, e))?;
    let orgs = api.organisations().await.map_err(|e| ApiError(StatusCode::UNAUTHORIZED, e))?;
    Ok(Json(json!({ "orgs": orgs })))
}

/// L'API d'un client pour une organisation : ses jetons (ou ceux du
/// serveur), l'organisation de `X-CC-Org` ou celle du serveur.
async fn api_of(s: &State, c: &Client) -> Result<crate::exec::platform::Api, ApiError> {
    let (api, _) = s.api_for(c).await.map_err(|e| ApiError(StatusCode::UNAUTHORIZED, e))?;
    let org = c.org.clone().or_else(|| s.fallback.as_ref().map(|f| f.org.clone()).filter(|o| !o.is_empty() && c.tokens.is_none()));
    let api = match org {
        Some(o) => api.for_owner(&o),
        None => api,
    };
    if api.owner().is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "aucune organisation choisie (en-tête X-CC-Org)".into()));
    }
    Ok(api)
}

// ---------------------------------------------------------------- plateforme

/// Un nom de projet reçu dans l'URL : la forme des noms Compose
/// (`normalize_project_name` : minuscules, chiffres, `-`, `_`, première
/// lettre alphanumérique). Il entre dans des clés S3, des chemins
/// temporaires et des tags : rien d'autre n'y passe.
fn project_param(project: &str) -> Result<String, ApiError> {
    let ok = !project.is_empty() && project.len() <= 128 && project.chars().next().is_some_and(|c| c.is_ascii_alphanumeric() && !c.is_ascii_uppercase()) && project.chars().all(|c| c.is_ascii_digit() || c.is_ascii_lowercase() || c == '-' || c == '_');
    if ok { Ok(project.to_string()) } else { Err(bad(format!("`{project}` : nom de projet invalide (minuscules, chiffres, `-`, `_`)"))) }
}

async fn platform_projects(AxState(s): AxState<Shared>, org: Client) -> Result<Json<J>, ApiError> {
    let api = api_of(&s, &org).await?;
    let projects = crate::exec::inspect::projects(&api).await.map_err(internal)?;
    Ok(Json(json!({ "projects": projects })))
}

/// L'état d'un projet sur la plateforme, dans une forme prête pour le
/// diagramme : applications (instances, dernier déploiement, domaines),
/// addons, network groups et leurs membres.
async fn inventory(AxState(s): AxState<Shared>, UrlPath(project): UrlPath<String>, org: Client) -> Result<Json<J>, ApiError> {
    let project = project_param(&project)?;
    let api = api_of(&s, &org).await?;
    let inv = crate::exec::inspect::discover(&project, &api).await.map_err(internal)?;
    let mut apps = Vec::new();
    for a in &inv.apps {
        let instances = api.instances(&a.info.id).await.unwrap_or_default();
        let last = api.last_deployment(&a.info.id).await.ok().flatten();
        let domains = if a.info.instance.lifetime == "TASK" { Vec::new() } else { api.list_domains(&a.info.id).await.unwrap_or_default() };
        apps.push(json!({
            "service": a.service,
            "id": a.info.id,
            "name": a.info.name,
            "task": a.info.instance.lifetime == "TASK",
            "tagged": a.tagged,
            "state": a.info.state,
            "instances": instances.iter().map(|i| json!({ "id": i.id, "state": i.state, "flavor": i.flavor.as_ref().map(|f| f.name.clone()) })).collect::<Vec<_>>(),
            "scaling": { "min_flavor": a.info.instance.min_flavor.name, "max_flavor": a.info.instance.max_flavor.name, "min_instances": a.info.instance.min_instances, "max_instances": a.info.instance.max_instances },
            "last_deployment": last.map(|d| json!({ "state": d.state, "id": d.uuid, "date": d.date })),
            "domains": domains.iter().map(|d| d.trim_end_matches('/')).collect::<Vec<_>>(),
            "tags": a.tags,
        }));
    }
    let addons: Vec<J> = inv.addons.iter().map(|a| json!({ "service": a.service, "id": a.info.id, "real_id": a.info.real_id, "name": a.info.name, "provider": a.info.provider, "plan": a.info.plan_slug })).collect();
    let ngs: Vec<J> = inv.ngs.iter().map(|n| json!({ "id": n.id, "label": n.label, "members": n.members.iter().map(|m| json!({ "id": m.id, "domain_name": m.domain_name })).collect::<Vec<_>>() })).collect();
    Ok(Json(json!({ "project": project, "apps": apps, "addons": addons, "network_groups": ngs })))
}

#[derive(Deserialize)]
struct LogsQuery {
    services: Option<String>,
    since: Option<String>,
    follow: Option<bool>,
    limit: Option<usize>,
}

async fn logs(AxState(s): AxState<Shared>, UrlPath(project): UrlPath<String>, org: Client, Query(q): Query<LogsQuery>) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let project = project_param(&project)?;
    let opts = crate::exec::inspect::LogsOptions { services: q.services.map(|v| v.split(',').filter(|x| !x.is_empty()).map(str::to_string).collect()).unwrap_or_default(), since: q.since, follow: q.follow.unwrap_or(false), limit: q.limit };
    Ok(stream_task(s, format!("logs {project}"), move |state, mut log| async move { crate::exec::inspect::logs(&project, &api_of(&state, &org).await.map_err(|e| e.1)?, &opts, &mut log).await }))
}

/// `ps` tel que la CLI l'affiche (exécution distante).
async fn ps(AxState(s): AxState<Shared>, UrlPath(project): UrlPath<String>, org: Client) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let project = project_param(&project)?;
    Ok(stream_task(s, format!("ps {project}"), move |state, mut log| async move { crate::exec::inspect::ps(&project, &api_of(&state, &org).await.map_err(|e| e.1)?, &mut log).await }))
}

#[derive(Deserialize, Default)]
struct DownBody {
    #[serde(default)]
    yes: bool,
    #[serde(default)]
    dry_run: bool,
}

async fn down(AxState(s): AxState<Shared>, UrlPath(project): UrlPath<String>, org: Client, body: Option<Json<DownBody>>) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let project = project_param(&project)?;
    let b = body.map(|Json(b)| b).unwrap_or_default();
    Ok(stream_task(s, format!("down {project}"), move |state, mut log| async move { crate::exec::reconcile::down(&project, &api_of(&state, &org).await.map_err(|e| e.1)?, b.dry_run, b.yes, &mut log).await }))
}

async fn stop(AxState(s): AxState<Shared>, UrlPath(project): UrlPath<String>, org: Client) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let project = project_param(&project)?;
    Ok(stream_task(s, format!("stop {project}"), move |state, mut log| async move { crate::exec::reconcile::stop_start(&project, &api_of(&state, &org).await.map_err(|e| e.1)?, false, &mut log).await }))
}

async fn start(AxState(s): AxState<Shared>, UrlPath(project): UrlPath<String>, org: Client) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let project = project_param(&project)?;
    Ok(stream_task(s, format!("start {project}"), move |state, mut log| async move { crate::exec::reconcile::stop_start(&project, &api_of(&state, &org).await.map_err(|e| e.1)?, true, &mut log).await }))
}

/// Manifeste, plan et fichiers texte du bucket d'assets du projet, sous
/// l'organisation du client (jetons validés, `X-CC-Org`) : le bucket est
/// celui du serveur, chacun n'y lit que le préfixe de son organisation.
async fn assets_get(AxState(s): AxState<Shared>, UrlPath(project): UrlPath<String>, c: Client) -> Result<Json<J>, ApiError> {
    let project = project_param(&project)?;
    let api = api_of(&s, &c).await?;
    let configured = std::env::var(crate::exec::assets::ENV_BUCKET).is_ok();
    let Some((cellar, manifest)) = crate::exec::assets::open(api.owner(), &project).await.map_err(internal)? else { return Ok(Json(json!({ "present": false, "configured": configured }))) };
    let (plan, files) = crate::exec::assets::fetch(&cellar, api.owner(), &manifest).await.map_err(internal)?;
    let files: Vec<J> = files.into_iter().map(|(path, bytes)| json!({ "path": path, "size": bytes.len(), "content": std::str::from_utf8(&bytes).ok() })).collect();
    Ok(Json(json!({ "present": true, "configured": true, "bucket": cellar.bucket_name(), "manifest": manifest, "plan": plan, "files": files })))
}

/// Rapatrie le bucket d'un projet dans un répertoire temporaire et
/// l'enregistre comme projet local.
async fn assets_open(AxState(s): AxState<Shared>, UrlPath(project): UrlPath<String>, c: Client) -> Result<Json<J>, ApiError> {
    let project = project_param(&project)?;
    let o = owner(&s, &c).await?;
    let api = api_of(&s, &c).await?;
    let Some((cellar, manifest)) = crate::exec::assets::open(api.owner(), &project).await.map_err(internal)? else { return Err(ApiError(StatusCode::NOT_FOUND, "pas d'assets pour ce projet".into())) };
    let (_plan, files) = crate::exec::assets::fetch(&cellar, api.owner(), &manifest).await.map_err(internal)?;
    // Un répertoire par rapatriement (deux clients, ou deux organisations,
    // peuvent ouvrir un projet du même nom) ; `project` est validé, la
    // jointure ne peut pas remplacer la racine.
    let root = super::local::fresh_temp_dir(&s.locals, &format!("assets-{project}")).map_err(internal)?;
    let dir = root.join(&project);
    for (path, bytes) in files {
        let Some(target) = safe_file(&dir, &path).ok() else { continue };
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| internal(e.to_string()))?;
        }
        std::fs::write(&target, bytes).map_err(|e| internal(e.to_string()))?;
    }
    let id = s.locals.add_dir(dir, true, o.as_deref()).map_err(internal)?;
    Ok(Json(json!({ "id": id })))
}

/// Lance `f` dans une tâche et diffuse son journal en SSE.
fn stream_task<F, Fut>(state: Shared, label: String, f: F) -> Sse<impl Stream<Item = Result<Event, Infallible>>>
where
    F: FnOnce(Shared, Log) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();
    let tx_log = tx.clone();
    // Chaque ligne va au client et au journal du serveur.
    let tag = label.clone();
    let log: Log = Box::new(move |line: &str| {
        if !line.starts_with(crate::ui::PROGRESS) {
            super::slog(&format!("  {tag} │ {line}"));
        }
        let _ = tx_log.send(("log".into(), line.to_string()));
    });
    super::slog(&format!("▶ {label}"));
    let started = std::time::Instant::now();
    tokio::spawn(async move {
        let result = f(state, log).await;
        let secs = started.elapsed().as_secs();
        match &result {
            Ok(()) => super::slog(&format!("✔ {label} terminé ({secs} s)")),
            Err(e) => super::slog(&format!("✘ {label} : {e} ({secs} s)")),
        }
        let _ = tx.send(("done".into(), match result {
            Ok(()) => "ok".into(),
            Err(e) => format!("error: {e}"),
        }));
    });
    let stream = UnboundedReceiverStream::new(rx).map(|(kind, data)| Ok(Event::default().event(kind).data(data)));
    Sse::new(stream).keep_alive(KeepAlive::default())
}

// ---------------------------------------------------------------- projets locaux

async fn local_list(AxState(s): AxState<Shared>, c: Client) -> Result<Json<J>, ApiError> {
    let o = owner(&s, &c).await?;
    let list: Vec<J> = s.locals.list(o.as_deref()).into_iter().map(|(id, dir, temporary)| json!({ "id": id, "dir": dir, "temporary": temporary, "name": project_name(&dir, temporary || o.is_some()) })).collect();
    Ok(Json(json!({ "projects": list })))
}

#[derive(Deserialize)]
struct AddDir {
    path: String,
}

/// Un répertoire du poste : réservé aux clients servis par les
/// identifiants du serveur (même machine) ; un client à jetons passe par
/// une archive.
async fn local_add_dir(AxState(s): AxState<Shared>, c: Client, Json(b): Json<AddDir>) -> Result<Json<J>, ApiError> {
    let o = owner(&s, &c).await?;
    if o.is_some() {
        return Err(ApiError(StatusCode::FORBIDDEN, "ouvrir un répertoire du serveur est réservé à l'usage local : envoyez une archive".into()));
    }
    let id = s.locals.add_dir(PathBuf::from(b.path), false, None).map_err(bad)?;
    Ok(Json(json!({ "id": id })))
}

async fn local_upload(AxState(s): AxState<Shared>, c: Client, mut multipart: Multipart) -> Result<Json<J>, ApiError> {
    let o = owner(&s, &c).await?;
    let Some(field) = multipart.next_field().await.map_err(|e| bad(e.to_string()))? else { return Err(bad("aucun fichier reçu")) };
    let name = field.file_name().map(str::to_string).unwrap_or_else(|| "archive.zip".into());
    let bytes = field.bytes().await.map_err(|e| bad(e.to_string()))?;
    let id = s.locals.add_archive(&name, &bytes, o.as_deref()).map_err(bad)?;
    super::slog(&format!("projet reçu : {name} ({} octets) → {id}{}", bytes.len(), o.map(|u| format!(" (de {u})")).unwrap_or_default()));
    Ok(Json(json!({ "id": id })))
}

async fn local_remove(AxState(s): AxState<Shared>, UrlPath(id): UrlPath<String>, c: Client) -> Result<Json<J>, ApiError> {
    let o = owner(&s, &c).await?;
    if s.locals.remove(&id, o.as_deref()) { super::slog(&format!("projet retiré : {id}")); Ok(Json(json!({ "ok": true }))) } else { Err(ApiError(StatusCode::NOT_FOUND, "projet inconnu".into())) }
}

/// Le propriétaire des projets pour cette requête (voir `State::owner_of`).
async fn owner(s: &State, c: &Client) -> Result<Option<String>, ApiError> {
    s.owner_of(c).await.map_err(|e| ApiError(StatusCode::UNAUTHORIZED, e))
}

/// L'exécution « locale » d'une tâche (`x-clever.init: local`, `run: local`,
/// `seed --local`) tourne sur la machine du serveur. Sur un serveur
/// partagé, un client ferait exécuter son script ici : elle exige que le
/// serveur la permette (`State::local_exec`) **et** que la requête vienne
/// du poste ([`Client::from_host`]) sans jeton propre (un client à jetons
/// est quelqu'un d'autre, même sur le poste). Aucun appel réseau :
/// décidable avant tout le reste.
pub fn local_execution_allowed(s: &State, c: &Client) -> bool {
    s.local_exec && c.tokens.is_none() && c.from_host
}

const LOCAL_EXECUTION_REFUSED: &str = "exécution locale réservée au poste : sur ce serveur, laissez la plateforme exécuter la tâche (retirez `x-clever.init: local`, ou décochez « exécuter depuis ce poste »)";

/// Un projet local visible du client : son répertoire, et s'il est
/// déposé (fichiers confinés au répertoire, `Options::confine`).
async fn project_dir(s: &State, id: &str, c: &Client) -> Result<(PathBuf, bool), ApiError> {
    let o = owner(s, c).await?;
    s.locals.get(id, o.as_deref()).ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("projet local `{id}` inconnu")))
}

fn project_name(dir: &Path, confine: bool) -> String {
    let opts = crate::compose::project::Options { project_dir: dir.to_path_buf(), files: Vec::new(), project_name: None, env_files: Vec::new(), profiles: Vec::new(), confine };
    let mut diags = Diagnostics::default();
    match crate::compose::project::load(&opts, &mut diags) {
        Some(p) => p.name,
        None => crate::compose::project::normalize_project_name(dir.file_name().and_then(|s| s.to_str()).unwrap_or("default")),
    }
}

/// Fichiers texte du projet (éditables), hors `.git`, `node_modules`,
/// `target` et fichiers de plus de 512 Kio.
fn text_files(dir: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
        if depth > 4 {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                if name == ".git" || name == "node_modules" || name == "target" || name == ".terraform" {
                    continue;
                }
                walk(root, &p, depth + 1, out);
            } else if p.is_file() && e.metadata().map(|m| m.len() <= 512 * 1024).unwrap_or(false) {
                if let Ok(bytes) = std::fs::read(&p) {
                    if std::str::from_utf8(&bytes).is_ok() && !bytes.contains(&0) {
                        out.push(p.strip_prefix(root).unwrap_or(&p).to_string_lossy().to_string());
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, 0, &mut out);
    // Compose d'abord, puis les `.env` (interpolation et `env_file`), puis le reste.
    out.sort_by_key(|f| (!f.to_ascii_lowercase().contains("compose"), !f.starts_with(".env"), f.clone()));
    out
}

async fn local_files(AxState(s): AxState<Shared>, UrlPath(id): UrlPath<String>, c: Client) -> Result<Json<J>, ApiError> {
    let (dir, confine) = project_dir(&s, &id, &c).await?;
    Ok(Json(json!({ "id": id, "dir": dir, "name": project_name(&dir, confine), "files": text_files(&dir) })))
}

#[derive(Deserialize)]
struct FileQuery {
    path: String,
}

fn safe_file(dir: &Path, rel: &str) -> Result<PathBuf, ApiError> {
    let p = Path::new(rel);
    if p.is_absolute() || p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(bad("chemin hors du projet"));
    }
    Ok(dir.join(p))
}

async fn local_file_get(AxState(s): AxState<Shared>, UrlPath(id): UrlPath<String>, c: Client, Query(q): Query<FileQuery>) -> Result<Json<J>, ApiError> {
    let (dir, _) = project_dir(&s, &id, &c).await?;
    let path = safe_file(&dir, &q.path)?;
    let content = std::fs::read_to_string(&path).map_err(|e| bad(format!("{} : {e}", q.path)))?;
    Ok(Json(json!({ "path": q.path, "content": content })))
}

#[derive(Deserialize)]
struct FileBody {
    path: String,
    content: String,
}

async fn local_file_put(AxState(s): AxState<Shared>, UrlPath(id): UrlPath<String>, c: Client, Json(b): Json<FileBody>) -> Result<Json<J>, ApiError> {
    let (dir, _) = project_dir(&s, &id, &c).await?;
    let path = safe_file(&dir, &b.path)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| internal(e.to_string()))?;
    }
    std::fs::write(&path, b.content).map_err(|e| internal(format!("{} : {e}", b.path)))?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize, Default)]
struct PlanQuery {
    #[serde(default)]
    allow_ephemeral: bool,
}

/// Plan et diagnostics d'un projet local, en JSON structuré.
/// Le résolveur d'images d'un projet : avec les identifiants Docker du
/// poste pour un répertoire du poste, sans pour un projet déposé.
fn registry(confine: bool) -> Result<crate::plan::registry::Registry, String> {
    if confine { crate::plan::registry::Registry::anonymous() } else { crate::plan::registry::Registry::new() }
}

fn compute(dir: &Path, confine: bool, allow_ephemeral: bool) -> Result<(Option<Plan>, J, PathBuf), String> {
    let resolver = registry(confine)?;
    let opts = crate::compose::project::Options { project_dir: dir.to_path_buf(), files: Vec::new(), project_name: None, env_files: Vec::new(), profiles: Vec::new(), confine };
    let project_dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let popts = crate::plan::cli::PlanOptions { project: opts, allow_ephemeral, private_registries: None };
    let mut diags = Diagnostics::default();
    let plan = crate::plan::cli::run(&popts, &resolver, &mut diags);
    let items: Vec<J> = diags
        .items
        .iter()
        .map(|d| {
            json!({
                "severity": if d.severity == Severity::Error { "error" } else { "warning" },
                "code": d.code,
                "message": d.message,
                "path": d.path.as_ref().map(|p| p.to_string()),
                "file": d.span.and_then(|sp| diags.files.0.get(sp.file).cloned()),
                "line": d.span.map(|sp| sp.line),
                "col": d.span.map(|sp| sp.col),
                "rendered": diags.files.render(d),
            })
        })
        .collect();
    Ok((plan, J::Array(items), project_dir))
}

async fn local_plan(AxState(s): AxState<Shared>, UrlPath(id): UrlPath<String>, c: Client, Query(q): Query<PlanQuery>) -> Result<Json<J>, ApiError> {
    let (dir, confine) = project_dir(&s, &id, &c).await?;
    let (plan, diags, _) = tokio::task::spawn_blocking(move || compute(&dir, confine, q.allow_ephemeral)).await.map_err(|e| internal(e.to_string()))?.map_err(internal)?;
    Ok(Json(json!({ "plan": plan, "diagnostics": diags })))
}

#[derive(Deserialize, Default)]
struct UpBody {
    #[serde(default)]
    yes: bool,
    #[serde(default)]
    dry_run: bool,
    #[serde(default)]
    no_wait: bool,
    #[serde(default)]
    allow_ephemeral: bool,
    #[serde(default)]
    assets: bool,
}

async fn local_up(AxState(s): AxState<Shared>, UrlPath(id): UrlPath<String>, org: Client, body: Option<Json<UpBody>>) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let local_ok = local_execution_allowed(&s, &org);
    let (dir, confine) = project_dir(&s, &id, &org).await?;
    let b = body.map(|Json(b)| b).unwrap_or_default();
    Ok(stream_task(s, format!("up {id}{}", if b.dry_run { " --dry-run" } else { "" }), move |state, mut log| async move {
        let (plan, diags, project_dir) = tokio::task::spawn_blocking(move || compute(&dir, confine, b.allow_ephemeral)).await.map_err(|e| e.to_string())??;
        for d in diags.as_array().into_iter().flatten() {
            log(&format!("{}{}", if d["severity"] == "error" { "erreur : " } else { "" }, d["rendered"].as_str().unwrap_or("")));
        }
        let plan = plan.filter(|_| !diags.as_array().into_iter().flatten().any(|d| d["severity"] == "error")).ok_or("plan refusé : corrigez les erreurs")?;
        if !local_ok {
            let local: Vec<&str> = plan.resources.iter().filter_map(|r| match r { cc_compose_model::plan::Resource::App(a) if a.run == cc_compose_model::plan::RunWhere::Local => Some(a.service.as_str()), _ => None }).collect();
            if !local.is_empty() {
                return Err(format!("{} : {LOCAL_EXECUTION_REFUSED}", local.join(", ")));
            }
        }
        let opts = crate::exec::reconcile::Options { project_dir: &project_dir, dry_run: b.dry_run, no_wait: b.no_wait, confirm_addons: b.yes, assets: b.assets };
        crate::exec::reconcile::up(&plan, &api_of(&state, &org).await.map_err(|e| e.1)?, &opts, &mut log).await.map(|_| ())
    }))
}

#[derive(Deserialize)]
struct SeedBody {
    service: String,
    /// Chemin du dump, relatif au projet.
    dump: String,
    #[serde(default)]
    local: bool,
    #[serde(default)]
    yes: bool,
    #[serde(default)]
    no_wait: bool,
}

async fn local_seed(AxState(s): AxState<Shared>, UrlPath(id): UrlPath<String>, org: Client, Json(b): Json<SeedBody>) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    if b.local && !local_execution_allowed(&s, &org) {
        return Err(ApiError(StatusCode::FORBIDDEN, LOCAL_EXECUTION_REFUSED.into()));
    }
    let (dir, confine) = project_dir(&s, &id, &org).await?;
    let dump = safe_file(&dir, &b.dump)?;
    Ok(stream_task(s, format!("seed {id} --service {}", b.service), move |state, mut log| async move {
        // Plan et plan de seed hors du runtime : `Registry` résout les
        // images avec son propre runtime (`block_on`), interdit depuis une
        // tâche Tokio.
        let (service, local) = (b.service.clone(), b.local);
        let (plan, project_dir) = tokio::task::spawn_blocking(move || -> Result<(Plan, PathBuf), String> {
            let (plan, _diags, project_dir) = compute(&dir, confine, false)?;
            let plan = plan.ok_or("plan refusé : corrigez les erreurs")?;
            let resolver = registry(confine)?;
            let plan = crate::plan::initdb::seed_plan(&plan, &service, &dump, &project_dir, &resolver, local)?;
            Ok((plan, project_dir))
        })
        .await
        .map_err(|e| e.to_string())??;
        let opts = crate::exec::reconcile::Options { project_dir: &project_dir, dry_run: false, no_wait: b.no_wait, confirm_addons: b.yes, assets: false };
        crate::exec::reconcile::up(&plan, &api_of(&state, &org).await.map_err(|e| e.1)?, &opts, &mut log).await.map(|_| ())
    }))
}

/// Reçoit un dump quelconque et le range dans le projet
/// (`.clever-compose/seed/<nom>`), puisque `seed` attend un fichier du
/// projet ; renvoie son chemin relatif.
async fn local_seed_upload(AxState(s): AxState<Shared>, UrlPath(id): UrlPath<String>, c: Client, mut multipart: Multipart) -> Result<Json<J>, ApiError> {
    let (dir, _) = project_dir(&s, &id, &c).await?;
    let Some(field) = multipart.next_field().await.map_err(|e| bad(e.to_string()))? else { return Err(bad("aucun fichier reçu")) };
    let name = field.file_name().map(str::to_string).unwrap_or_else(|| "dump".into());
    let name = Path::new(&name).file_name().and_then(|n| n.to_str()).unwrap_or("dump").to_string();
    let bytes = field.bytes().await.map_err(|e| bad(e.to_string()))?;
    let rel = format!(".clever-compose/seed/{name}");
    let path = dir.join(&rel);
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| internal(e.to_string()))?;
    std::fs::write(&path, &bytes).map_err(|e| internal(format!("{rel} : {e}")))?;
    Ok(Json(json!({ "path": rel, "size": bytes.len() })))
}

/// Flavors Docker de la plateforme (catalogue embarqué).
async fn flavors() -> Json<J> {
    let d = crate::plan::flavors::docker();
    Json(json!({ "default": d.default_flavor, "max_instances": d.max_instances, "flavors": d.flavors.iter().map(|f| json!({ "name": f.name, "cpus": f.cpus, "memory_bytes": f.memory_bytes })).collect::<Vec<_>>() }))
}

#[derive(Deserialize)]
struct ScalingBody {
    service: String,
    min_flavor: Option<String>,
    max_flavor: Option<String>,
    min_instances: Option<u32>,
    max_instances: Option<u32>,
}

/// Le fichier compose principal du projet (celui que la découverte prend).
fn main_compose(dir: &Path) -> Option<PathBuf> {
    ["compose.yaml", "compose.yml", "docker-compose.yaml", "docker-compose.yml"].iter().map(|f| dir.join(f)).find(|p| p.is_file())
}

/// Pose la scalabilité d'un service dans `x-clever` du compose, sans
/// toucher au reste du fichier ; la CLI et `up` la reprennent ensuite.
async fn local_scaling(AxState(s): AxState<Shared>, UrlPath(id): UrlPath<String>, c: Client, Json(b): Json<ScalingBody>) -> Result<Json<J>, ApiError> {
    let (dir, _) = project_dir(&s, &id, &c).await?;
    let path = main_compose(&dir).ok_or_else(|| bad("aucun fichier compose dans le projet"))?;
    let text = std::fs::read_to_string(&path).map_err(|e| internal(e.to_string()))?;
    let d = crate::plan::flavors::docker();
    for f in [&b.min_flavor, &b.max_flavor].into_iter().flatten() {
        if !d.flavors.iter().any(|x| &x.name == f) {
            return Err(bad(format!("flavor `{f}` inconnue")));
        }
    }
    if let (Some(lo), Some(hi)) = (b.min_instances, b.max_instances) {
        if lo > hi {
            return Err(bad("min_instances dépasse max_instances"));
        }
    }
    use crate::compose::edit::Entry;
    let same_flavor = b.min_flavor.is_some() && b.min_flavor == b.max_flavor;
    let entries = vec![
        // `flavor` = raccourci quand min = max ; sinon les deux bornes.
        Entry { key: "flavor", value: if same_flavor { b.min_flavor.clone() } else { None } },
        Entry { key: "min_flavor", value: if same_flavor { None } else { b.min_flavor.clone() } },
        Entry { key: "max_flavor", value: if same_flavor { None } else { b.max_flavor.clone() } },
        Entry { key: "min_instances", value: b.min_instances.map(|v| v.to_string()) },
        Entry { key: "max_instances", value: b.max_instances.map(|v| v.to_string()) },
    ];
    let out = crate::compose::edit::set_xclever(&text, &b.service, &entries).map_err(bad)?;
    std::fs::write(&path, &out).map_err(|e| internal(e.to_string()))?;
    Ok(Json(json!({ "ok": true, "path": path.strip_prefix(&dir).unwrap_or(&path).to_string_lossy() })))
}

/// Pousse les assets du projet dans le bucket configuré, sans `up`.
async fn local_assets_sync(AxState(s): AxState<Shared>, UrlPath(id): UrlPath<String>, c: Client) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let (dir, confine) = project_dir(&s, &id, &c).await?;
    Ok(stream_task(s, format!("assets {id}"), move |state, mut log| async move {
        let api = api_of(&state, &c).await.map_err(|e| e.1)?;
        let (plan, diags, project_dir) = tokio::task::spawn_blocking(move || compute(&dir, confine, false)).await.map_err(|e| e.to_string())??;
        let plan = plan.filter(|_| !diags.as_array().into_iter().flatten().any(|d| d["severity"] == "error")).ok_or("plan refusé : corrigez les erreurs")?;
        let opts = crate::exec::reconcile::Options { project_dir: &project_dir, dry_run: false, no_wait: true, confirm_addons: false, assets: true };
        crate::exec::assets::sync(api.owner(), &plan, &project_dir, &opts, &mut log).await
    }))
}

#[derive(Deserialize, Default)]
struct ExportBody {
    #[serde(default)]
    branches: bool,
    #[serde(default)]
    allow_ephemeral: bool,
}

/// Export Terraform + script, renvoyé en fichiers (contenu texte).
async fn local_export(AxState(s): AxState<Shared>, UrlPath(id): UrlPath<String>, c: Client, body: Option<Json<ExportBody>>) -> Result<Json<J>, ApiError> {
    let (dir, confine) = project_dir(&s, &id, &c).await?;
    let b = body.map(|Json(b)| b).unwrap_or_default();
    let result = tokio::task::spawn_blocking(move || -> Result<J, String> {
        let (plan, diags, project_dir) = compute(&dir, confine, b.allow_ephemeral)?;
        let plan = plan.filter(|_| !diags.as_array().into_iter().flatten().any(|d| d["severity"] == "error")).ok_or("plan refusé : corrigez les erreurs")?;
        let out = std::env::temp_dir().join(format!("clever-compose-export-{}-{}", std::process::id(), plan.spec_hash));
        let _ = std::fs::remove_dir_all(&out);
        let what = crate::emit::export::What { terraform: true, script: true, repos: false, branches: b.branches };
        let mut ediags = Diagnostics::default();
        crate::emit::export::write(&plan, &project_dir, &out, &what, &mut ediags).map_err(|d| d.message)?;
        let mut files = Vec::new();
        for f in text_files(&out) {
            let content = std::fs::read_to_string(out.join(&f)).unwrap_or_default();
            files.push(json!({ "path": f, "content": content }));
        }
        let _ = std::fs::remove_dir_all(&out);
        let warnings: Vec<String> = ediags.items.iter().map(|d| ediags.files.render(d)).collect();
        Ok(json!({ "files": files, "diagnostics": warnings }))
    })
    .await
    .map_err(|e| internal(e.to_string()))?
    .map_err(bad)?;
    Ok(Json(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::Request;
    use std::net::SocketAddr;
    use tower::ServiceExt;

    const LOOPBACK: &str = "127.0.0.1:41000";
    const REMOTE: &str = "203.0.113.7:41000";

    /// Un serveur du poste : identifiants de repli, écoute locale, un
    /// projet local enregistré. Aucun appel réseau tant que les jetons du
    /// client ne sont pas validés (ils ne le sont jamais ici).
    fn state(local_exec: bool) -> (Shared, String, tempdir::Dir) {
        let dir = tempdir::Dir::new();
        std::fs::write(dir.path.join("compose.yaml"), "services:\n  web:\n    image: nginx\n").unwrap();
        let api = crate::exec::platform::Api::new("orga_test".into(), clevercloud_sdk::Credentials::Bearer { token: "t".into() });
        let s = State {
            fallback: Some(super::super::Session { api, org: "orga_test".into() }),
            locals: super::super::local::Locals::default(),
            oauth: super::super::OauthConsumer { key: "k".into(), secret: "s".into() },
            public_base: None,
            listen: "127.0.0.1:8080".parse().unwrap(),
            origins: Vec::new(),
            pending: Default::default(),
            api_key: None,
            local_exec,
            validated: Default::default(),
        };
        let id = s.locals.add_dir(dir.path.clone(), false, None).unwrap();
        (Arc::new(s), id, dir)
    }

    fn client(tokens: bool, from_host: bool) -> Client {
        Client { tokens: tokens.then(|| super::super::ClientTokens::Bearer("x".into())), org: None, from_host }
    }

    /// Une requête prête à partir : pair, en-têtes, jetons, corps JSON
    /// facultatif.
    fn request(method: &str, uri: &str, peer: &str, headers: &[(&str, &str)], tokens: bool, body: Option<String>) -> Request<Body> {
        let mut req = Request::builder().method(method).uri(uri).header("host", "127.0.0.1:8080");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        if tokens {
            req = req.header("x-cc-api-token", "x");
        }
        let mut req = match body {
            Some(b) => req.header("content-type", "application/json").body(Body::from(b)).unwrap(),
            None => req.body(Body::empty()).unwrap(),
        };
        req.extensions_mut().insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
        req
    }

    /// Le routeur avec les mêmes couches que `serve`.
    fn app(s: &Shared) -> axum::Router {
        router(s.clone()).layer(axum::middleware::from_fn_with_state(s.clone(), require_key)).layer(axum::middleware::from_fn_with_state(s.clone(), same_site)).layer(axum::middleware::from_fn_with_state(s.clone(), cors))
    }

    async fn seed(s: &Shared, id: &str, peer: &str, headers: &[(&str, &str)], tokens: bool, dump: &str) -> StatusCode {
        let req = request("POST", &format!("/api/local/{id}/seed"), peer, headers, tokens, Some(format!(r#"{{"service":"db","dump":"{dump}","local":true}}"#)));
        app(s).oneshot(req).await.unwrap().status()
    }

    #[test]
    fn from_host_requires_loopback_peer_without_proxy_headers() {
        let none = axum::http::HeaderMap::new();
        let mut proxied = axum::http::HeaderMap::new();
        proxied.insert("x-forwarded-for", "198.51.100.4".parse().unwrap());
        assert!(from_host(&none, Some(LOOPBACK.parse().unwrap())));
        assert!(!from_host(&none, Some(REMOTE.parse().unwrap())));
        assert!(!from_host(&proxied, Some(LOOPBACK.parse().unwrap())));
        assert!(!from_host(&none, None));
    }

    #[test]
    fn allowed_only_from_the_host_without_tokens() {
        let (s, _, _d) = state(true);
        assert!(local_execution_allowed(&s, &client(false, true)));
        assert!(!local_execution_allowed(&s, &client(true, true)));
        assert!(!local_execution_allowed(&s, &client(false, false)));
        let (off, _, _d2) = state(false);
        assert!(!local_execution_allowed(&off, &client(false, true)));
    }

    /// Les identifiants du serveur ne sont prêtés qu'aux requêtes du poste.
    #[tokio::test]
    async fn server_credentials_only_from_the_host() {
        let (s, _, _d) = state(true);
        let list = |peer: &'static str, headers: &'static [(&'static str, &'static str)]| {
            let s = s.clone();
            async move { app(&s).oneshot(request("GET", "/api/local", peer, headers, false, None)).await.unwrap().status() }
        };
        assert_eq!(list(LOOPBACK, &[]).await, StatusCode::OK);
        assert_eq!(list(REMOTE, &[]).await, StatusCode::UNAUTHORIZED);
        assert_eq!(list(LOOPBACK, &[("x-forwarded-for", "198.51.100.4")]).await, StatusCode::UNAUTHORIZED);
    }

    /// Les assets d'un projet ne se lisent qu'authentifié (organisation
    /// du client) : sans identifiants, 401 avant tout accès au bucket.
    #[tokio::test]
    async fn assets_require_a_client() {
        let (s, _, _d) = state(true);
        let get = |peer: &'static str| {
            let s = s.clone();
            async move { app(&s).oneshot(request("GET", "/api/platform/projects/demo/assets", peer, &[], false, None)).await.unwrap().status() }
        };
        assert_eq!(get(REMOTE).await, StatusCode::UNAUTHORIZED);
        // Depuis le poste : identifiants du serveur, organisation du serveur ;
        // assets non configurés dans ce test → `present: false`.
        assert_eq!(get(LOOPBACK).await, StatusCode::OK);
    }

    /// Un nom de projet de l'URL entre dans des clés S3 et des chemins :
    /// seule la forme Compose passe (percent-décodage d'axum compris).
    #[tokio::test]
    async fn project_name_in_url_is_validated() {
        let (s, _, _d) = state(true);
        let get = |uri: &'static str| {
            let s = s.clone();
            async move { app(&s).oneshot(request("GET", uri, LOOPBACK, &[], false, None)).await.unwrap().status() }
        };
        assert_eq!(get("/api/platform/projects/..%2F..%2Fetc/assets").await, StatusCode::BAD_REQUEST);
        assert_eq!(get("/api/platform/projects/%2Fsrv/assets").await, StatusCode::BAD_REQUEST);
        assert_eq!(get("/api/platform/projects/Demo/assets").await, StatusCode::BAD_REQUEST);
        assert_eq!(get("/api/platform/projects/demo-1_a/assets").await, StatusCode::OK);
        assert!(project_param("").is_err());
        assert!(project_param("-x").is_err());
    }

    /// Anti-CSRF et anti-rebinding : `Host` inconnu, `Origin` étrangère,
    /// `Sec-Fetch-Site: cross-site`.
    #[tokio::test]
    async fn same_site_rejects_foreign_host_and_origin() {
        let (s, id, _d) = state(true);
        let call = |method: &'static str, uri: String, headers: Vec<(&'static str, &'static str)>| {
            let s = s.clone();
            async move {
                let mut req = request(method, &uri, LOOPBACK, &[], false, if method == "GET" { None } else { Some("{}".into()) });
                for (k, v) in headers {
                    if k == "host" {
                        req.headers_mut().remove("host");
                    }
                    req.headers_mut().append(k, v.parse().unwrap());
                }
                app(&s).oneshot(req).await.unwrap().status()
            }
        };
        // Host : rebinding DNS vers 127.0.0.1.
        assert_eq!(call("GET", "/api/local".into(), vec![("host", "attaquant.example")]).await, StatusCode::MISDIRECTED_REQUEST);
        assert_eq!(call("GET", "/api/local".into(), vec![("host", "localhost:8080")]).await, StatusCode::OK);
        assert_eq!(call("GET", "/api/local".into(), vec![("host", "LOCALHOST:8080")]).await, StatusCode::OK);
        // Origin : POST cross-origin (fetch no-cors, formulaire).
        let down = format!("/api/local/{id}/scaling");
        assert_eq!(call("PUT", down.clone(), vec![("origin", "https://attaquant.example")]).await, StatusCode::FORBIDDEN);
        assert_eq!(call("PUT", down.clone(), vec![("sec-fetch-site", "cross-site")]).await, StatusCode::FORBIDDEN);
        // Même origine : la requête passe la garde (400 : corps incomplet).
        assert_eq!(call("PUT", down.clone(), vec![("origin", "http://127.0.0.1:8080"), ("sec-fetch-site", "same-origin")]).await, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(call("PUT", down, vec![]).await, StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn seed_local_refused_unless_from_the_host() {
        let (s, id, _d) = state(true);
        // Client à jetons (refus avant toute validation réseau des jetons).
        assert_eq!(seed(&s, &id, LOOPBACK, &[], true, "dump.sql").await, StatusCode::FORBIDDEN);
        // Pair non loopback (écoute publique).
        assert_eq!(seed(&s, &id, REMOTE, &[], false, "dump.sql").await, StatusCode::FORBIDDEN);
        // Loopback derrière un reverse proxy.
        assert_eq!(seed(&s, &id, LOOPBACK, &[("x-forwarded-for", "198.51.100.4")], false, "dump.sql").await, StatusCode::FORBIDDEN);
        assert_eq!(seed(&s, &id, LOOPBACK, &[("forwarded", "for=198.51.100.4")], false, "dump.sql").await, StatusCode::FORBIDDEN);
        // Serveur configuré publié (`--local-exec off` ou indice de publication).
        let (off, id2, _d2) = state(false);
        assert_eq!(seed(&off, &id2, LOOPBACK, &[], false, "dump.sql").await, StatusCode::FORBIDDEN);
        // Depuis le poste : la garde passe, la requête échoue plus loin
        // (chemin de dump hors du projet), sans rien exécuter.
        assert_eq!(seed(&s, &id, LOOPBACK, &[], false, "../x").await, StatusCode::BAD_REQUEST);
    }

    mod tempdir {
        pub struct Dir {
            pub path: std::path::PathBuf,
        }
        impl Dir {
            pub fn new() -> Dir {
                static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
                let path = std::env::temp_dir().join(format!("cc-compose-test-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
                std::fs::create_dir_all(&path).unwrap();
                Dir { path: std::fs::canonicalize(&path).unwrap() }
            }
        }
        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.path);
            }
        }
    }
}
