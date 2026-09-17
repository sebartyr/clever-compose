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
            h.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("Content-Type, Authorization, X-CC-Token, X-CC-Secret, X-CC-Consumer-Key, X-CC-Consumer-Secret, X-CC-Api-Token, X-CC-Org"));
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
                return ApiError(StatusCode::UNAUTHORIZED, "clé d'API absente ou invalide (--api-key du serveur, CC_API_KEY côté client)".into()).into_response();
            }
        }
    }
    next.run(req).await
}

pub fn router(state: Shared) -> Router {
    Router::new()
        .route("/api/info", get(info))
        .route("/api/session", get(session_get))
        .route("/api/session/login", post(login_start))
        .route("/api/session/login/{request_token}", get(login_poll))
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
    let stamp = env!("CC_BUILD_STAMP");
    let (commit, secs) = stamp.split_once(' ').unwrap_or((stamp, "0"));
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"), "commit": commit, "built": crate::exec::inspect::rfc3339(secs.parse().unwrap_or(0)),
        "bundled": cfg!(feature = "ui-bundled"), "assets_bucket": std::env::var(crate::exec::assets::ENV_BUCKET).ok(), "api_key": s.api_key.is_some(),
        // Le serveur a-t-il ses propres identifiants (clients sans jetons servis) ?
        "server_credentials": s.fallback.is_some(),
    }))
}

/// Jetons et organisation de la requête. `X-CC-Token` + `X-CC-Secret`
/// (OAuth 1.0, ceux de `login` ou de l'environnement du client, avec
/// `X-CC-Consumer-Key`/`X-CC-Consumer-Secret` du consumer qui les a émis)
/// ou `X-CC-Api-Token` (bearer) ; `X-CC-Org` pour l'organisation visée.
#[derive(Clone, Default)]
pub struct Client {
    pub tokens: Option<super::ClientTokens>,
    pub org: Option<String>,
}

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for Client {
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(parts: &mut axum::http::request::Parts, _s: &S) -> Result<Self, Self::Rejection> {
        let h = |name: &str| parts.headers.get(name).and_then(|v| v.to_str().ok()).map(str::trim).filter(|v| !v.is_empty()).map(str::to_string);
        let consumer = match (h("x-cc-consumer-key"), h("x-cc-consumer-secret")) {
            (Some(k), Some(s)) => Some((k, s)),
            _ => None,
        };
        let tokens = match (h("x-cc-token"), h("x-cc-secret"), h("x-cc-api-token")) {
            (Some(token), Some(secret), _) => Some(super::ClientTokens::OAuth1 { token, secret, consumer }),
            (_, _, Some(t)) => Some(super::ClientTokens::Bearer(t)),
            _ => None,
        };
        Ok(Client { tokens, org: h("x-cc-org") })
    }
}

/// Qui est le client : jetons valides ?, utilisateur, organisation demandée.
/// `authenticated: false` avec `reason` quand les jetons sont refusés :
/// l'interface propose alors la connexion.
async fn session_get(AxState(s): AxState<Shared>, c: Client) -> Json<J> {
    match s.api_for(c.tokens.as_ref()).await {
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
    /// Une CLI : pas de retour, elle interroge `/api/session/login/{request_token}`.
    #[serde(default)]
    cli: bool,
}

/// Début de la connexion d'un navigateur : request token avec callback
/// sur ce serveur, URL d'autorisation à ouvrir. Le callback renvoie les
/// jetons dans le fragment de l'URL de retour.
async fn login_start(AxState(s): AxState<Shared>, body: Option<Json<OauthStart>>) -> Result<Json<J>, ApiError> {
    let q = body.map(|Json(b)| b).unwrap_or(OauthStart { return_to: None, cli: false });
    let return_to = if q.cli {
        None
    } else {
        Some(match q.return_to.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
            None => "/".to_string(),
            Some(r) if r.starts_with('/') && !r.starts_with("//") => r.to_string(),
            Some(r) if s.origins.iter().chain(std::iter::once(&s.public_base)).any(|o| r == o || r.starts_with(&format!("{o}/"))) => r.to_string(),
            Some(r) => return Err(bad(format!("`return={r}` : ni un chemin de ce serveur ni une origine passée en --cors"))),
        })
    };
    let callback = format!("{}/api/session/oauth/callback", s.public_base);
    let (token, secret, url) = crate::auth::request_token(&s.oauth, &callback).await.map_err(internal)?;
    s.pending.lock().unwrap().insert(token.clone(), (secret, return_to));
    Ok(Json(json!({ "mode": "redirect", "url": url, "request_token": token })))
}

/// Une CLI vient chercher ses jetons : `{ready: false}` tant que le
/// navigateur n'est pas passé par le callback ; rendus une seule fois.
async fn login_poll(AxState(s): AxState<Shared>, UrlPath(request_token): UrlPath<String>) -> Result<Json<J>, ApiError> {
    let mut map = s.cli_tokens.lock().unwrap();
    map.retain(|_, (_, at)| at.elapsed() < std::time::Duration::from_secs(600));
    match map.remove(&request_token) {
        Some((t, _)) => Ok(Json(json!({ "ready": true, "token": t.token, "secret": t.secret, "email": t.email, "consumer_key": s.oauth.key }))),
        None if s.pending.lock().unwrap().contains_key(&request_token) => Ok(Json(json!({ "ready": false }))),
        None => Err(ApiError(StatusCode::NOT_FOUND, "request token inconnu, expiré ou déjà consommé".into())),
    }
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
    let mut stored = crate::auth::Stored { token: token.clone(), secret: secret.clone(), consumer_key: None, api_url: None, user_id: None, email: None };
    crate::auth::verify(&mut stored, &s.oauth).await.map_err(|e| ApiError(StatusCode::UNAUTHORIZED, e))?;
    match return_to {
        Some(r) => Ok(axum::response::Redirect::temporary(&format!("{r}#token={}&secret={}", oauth_enc(&token), oauth_enc(&secret))).into_response()),
        None => {
            s.cli_tokens.lock().unwrap().insert(q.oauth_token.clone(), (stored, std::time::Instant::now()));
            Ok(axum::response::Html("<!doctype html><meta charset=utf-8><title>clever-compose</title><p>Accès autorisé : vous pouvez fermer cet onglet et revenir au terminal.").into_response())
        }
    }
}

async fn orgs(AxState(s): AxState<Shared>, c: Client) -> Result<Json<J>, ApiError> {
    let (api, _) = s.api_for(c.tokens.as_ref()).await.map_err(|e| ApiError(StatusCode::UNAUTHORIZED, e))?;
    let orgs = api.organisations().await.map_err(|e| ApiError(StatusCode::UNAUTHORIZED, e))?;
    Ok(Json(json!({ "orgs": orgs })))
}

/// L'API d'un client pour une organisation : ses jetons (ou ceux du
/// serveur), l'organisation de `X-CC-Org` ou celle du serveur.
async fn api_of(s: &State, c: &Client) -> Result<crate::exec::platform::Api, ApiError> {
    let (api, _) = s.api_for(c.tokens.as_ref()).await.map_err(|e| ApiError(StatusCode::UNAUTHORIZED, e))?;
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

async fn platform_projects(AxState(s): AxState<Shared>, org: Client) -> Result<Json<J>, ApiError> {
    let api = api_of(&s, &org).await?;
    let projects = crate::exec::inspect::projects(&api).await.map_err(internal)?;
    Ok(Json(json!({ "projects": projects })))
}

/// L'état d'un projet sur la plateforme, dans une forme prête pour le
/// diagramme : applications (instances, dernier déploiement, domaines),
/// addons, network groups et leurs membres.
async fn inventory(AxState(s): AxState<Shared>, UrlPath(project): UrlPath<String>, org: Client) -> Result<Json<J>, ApiError> {
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

async fn logs(AxState(s): AxState<Shared>, UrlPath(project): UrlPath<String>, org: Client, Query(q): Query<LogsQuery>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let opts = crate::exec::inspect::LogsOptions { services: q.services.map(|v| v.split(',').filter(|x| !x.is_empty()).map(str::to_string).collect()).unwrap_or_default(), since: q.since, follow: q.follow.unwrap_or(false), limit: q.limit };
    stream_task(s, move |state, mut log| async move { crate::exec::inspect::logs(&project, &api_of(&state, &org).await.map_err(|e| e.1)?, &opts, &mut log).await })
}

/// `ps` tel que la CLI l'affiche (exécution distante).
async fn ps(AxState(s): AxState<Shared>, UrlPath(project): UrlPath<String>, org: Client) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    stream_task(s, move |state, mut log| async move { crate::exec::inspect::ps(&project, &api_of(&state, &org).await.map_err(|e| e.1)?, &mut log).await })
}

#[derive(Deserialize, Default)]
struct DownBody {
    #[serde(default)]
    yes: bool,
    #[serde(default)]
    dry_run: bool,
}

async fn down(AxState(s): AxState<Shared>, UrlPath(project): UrlPath<String>, org: Client, body: Option<Json<DownBody>>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let b = body.map(|Json(b)| b).unwrap_or_default();
    stream_task(s, move |state, mut log| async move { crate::exec::reconcile::down(&project, &api_of(&state, &org).await.map_err(|e| e.1)?, b.dry_run, b.yes, &mut log).await })
}

async fn stop(AxState(s): AxState<Shared>, UrlPath(project): UrlPath<String>, org: Client) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    stream_task(s, move |state, mut log| async move { crate::exec::reconcile::stop_start(&project, &api_of(&state, &org).await.map_err(|e| e.1)?, false, &mut log).await })
}

async fn start(AxState(s): AxState<Shared>, UrlPath(project): UrlPath<String>, org: Client) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    stream_task(s, move |state, mut log| async move { crate::exec::reconcile::stop_start(&project, &api_of(&state, &org).await.map_err(|e| e.1)?, true, &mut log).await })
}

/// Manifeste, plan et fichiers texte du bucket d'assets du projet.
async fn assets_get(AxState(_s): AxState<Shared>, UrlPath(project): UrlPath<String>) -> Result<Json<J>, ApiError> {
    let configured = std::env::var(crate::exec::assets::ENV_BUCKET).is_ok();
    let Some((cellar, manifest)) = crate::exec::assets::open(&project).await.map_err(internal)? else { return Ok(Json(json!({ "present": false, "configured": configured }))) };
    let (plan, files) = crate::exec::assets::fetch(&cellar, &manifest).await.map_err(internal)?;
    let files: Vec<J> = files.into_iter().map(|(path, bytes)| json!({ "path": path, "size": bytes.len(), "content": std::str::from_utf8(&bytes).ok() })).collect();
    Ok(Json(json!({ "present": true, "configured": true, "bucket": cellar.bucket_name(), "manifest": manifest, "plan": plan, "files": files })))
}

/// Rapatrie le bucket d'un projet dans un répertoire temporaire et
/// l'enregistre comme projet local.
async fn assets_open(AxState(s): AxState<Shared>, UrlPath(project): UrlPath<String>) -> Result<Json<J>, ApiError> {
    let Some((cellar, manifest)) = crate::exec::assets::open(&project).await.map_err(internal)? else { return Err(ApiError(StatusCode::NOT_FOUND, "pas d'assets pour ce projet".into())) };
    let (_plan, files) = crate::exec::assets::fetch(&cellar, &manifest).await.map_err(internal)?;
    let root = std::env::temp_dir().join(format!("clever-compose-ui-{}-assets-{project}", std::process::id()));
    let dir = root.join(&project);
    let _ = std::fs::remove_dir_all(&root);
    for (path, bytes) in files {
        let Some(target) = safe_file(&dir, &path).ok() else { continue };
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| internal(e.to_string()))?;
        }
        std::fs::write(&target, bytes).map_err(|e| internal(e.to_string()))?;
    }
    let id = s.locals.add_dir(dir, true).map_err(internal)?;
    Ok(Json(json!({ "id": id })))
}

/// Lance `f` dans une tâche et diffuse son journal en SSE.
fn stream_task<F, Fut>(state: Shared, f: F) -> Sse<impl Stream<Item = Result<Event, Infallible>>>
where
    F: FnOnce(Shared, Log) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<(), String>> + Send + 'static,
{
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<(String, String)>();
    let tx_log = tx.clone();
    let log: Log = Box::new(move |line: &str| {
        let _ = tx_log.send(("log".into(), line.to_string()));
    });
    tokio::spawn(async move {
        let result = f(state, log).await;
        let _ = tx.send(("done".into(), match result {
            Ok(()) => "ok".into(),
            Err(e) => format!("error: {e}"),
        }));
    });
    let stream = UnboundedReceiverStream::new(rx).map(|(kind, data)| Ok(Event::default().event(kind).data(data)));
    Sse::new(stream).keep_alive(KeepAlive::default())
}

// ---------------------------------------------------------------- projets locaux

async fn local_list(AxState(s): AxState<Shared>) -> Json<J> {
    let list: Vec<J> = s.locals.list().into_iter().map(|(id, dir, temporary)| json!({ "id": id, "dir": dir, "temporary": temporary, "name": project_name(&dir) })).collect();
    Json(json!({ "projects": list }))
}

#[derive(Deserialize)]
struct AddDir {
    path: String,
}

async fn local_add_dir(AxState(s): AxState<Shared>, Json(b): Json<AddDir>) -> Result<Json<J>, ApiError> {
    let id = s.locals.add_dir(PathBuf::from(b.path), false).map_err(bad)?;
    Ok(Json(json!({ "id": id })))
}

async fn local_upload(AxState(s): AxState<Shared>, mut multipart: Multipart) -> Result<Json<J>, ApiError> {
    let Some(field) = multipart.next_field().await.map_err(|e| bad(e.to_string()))? else { return Err(bad("aucun fichier reçu")) };
    let name = field.file_name().map(str::to_string).unwrap_or_else(|| "archive.zip".into());
    let bytes = field.bytes().await.map_err(|e| bad(e.to_string()))?;
    let id = s.locals.add_archive(&name, &bytes).map_err(bad)?;
    Ok(Json(json!({ "id": id })))
}

async fn local_remove(AxState(s): AxState<Shared>, UrlPath(id): UrlPath<String>) -> Result<Json<J>, ApiError> {
    if s.locals.remove(&id) { Ok(Json(json!({ "ok": true }))) } else { Err(ApiError(StatusCode::NOT_FOUND, "projet inconnu".into())) }
}

fn project_dir(s: &State, id: &str) -> Result<PathBuf, ApiError> {
    s.locals.get(id).ok_or_else(|| ApiError(StatusCode::NOT_FOUND, format!("projet local `{id}` inconnu")))
}

fn project_name(dir: &Path) -> String {
    let opts = crate::compose::project::Options { project_dir: dir.to_path_buf(), files: Vec::new(), project_name: None, env_files: Vec::new(), profiles: Vec::new() };
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

async fn local_files(AxState(s): AxState<Shared>, UrlPath(id): UrlPath<String>) -> Result<Json<J>, ApiError> {
    let dir = project_dir(&s, &id)?;
    Ok(Json(json!({ "id": id, "dir": dir, "name": project_name(&dir), "files": text_files(&dir) })))
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

async fn local_file_get(AxState(s): AxState<Shared>, UrlPath(id): UrlPath<String>, Query(q): Query<FileQuery>) -> Result<Json<J>, ApiError> {
    let dir = project_dir(&s, &id)?;
    let path = safe_file(&dir, &q.path)?;
    let content = std::fs::read_to_string(&path).map_err(|e| bad(format!("{} : {e}", q.path)))?;
    Ok(Json(json!({ "path": q.path, "content": content })))
}

#[derive(Deserialize)]
struct FileBody {
    path: String,
    content: String,
}

async fn local_file_put(AxState(s): AxState<Shared>, UrlPath(id): UrlPath<String>, Json(b): Json<FileBody>) -> Result<Json<J>, ApiError> {
    let dir = project_dir(&s, &id)?;
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
fn compute(dir: &Path, allow_ephemeral: bool) -> Result<(Option<Plan>, J, PathBuf), String> {
    let resolver = crate::plan::registry::Registry::new()?;
    let opts = crate::compose::project::Options { project_dir: dir.to_path_buf(), files: Vec::new(), project_name: None, env_files: Vec::new(), profiles: Vec::new() };
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

async fn local_plan(AxState(s): AxState<Shared>, UrlPath(id): UrlPath<String>, Query(q): Query<PlanQuery>) -> Result<Json<J>, ApiError> {
    let dir = project_dir(&s, &id)?;
    let (plan, diags, _) = tokio::task::spawn_blocking(move || compute(&dir, q.allow_ephemeral)).await.map_err(|e| internal(e.to_string()))?.map_err(internal)?;
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
    let dir = project_dir(&s, &id)?;
    let b = body.map(|Json(b)| b).unwrap_or_default();
    Ok(stream_task(s, move |state, mut log| async move {
        let (plan, diags, project_dir) = tokio::task::spawn_blocking(move || compute(&dir, b.allow_ephemeral)).await.map_err(|e| e.to_string())??;
        for d in diags.as_array().into_iter().flatten() {
            log(&format!("{}{}", if d["severity"] == "error" { "erreur : " } else { "" }, d["rendered"].as_str().unwrap_or("")));
        }
        let plan = plan.filter(|_| !diags.as_array().into_iter().flatten().any(|d| d["severity"] == "error")).ok_or("plan refusé : corrigez les erreurs")?;
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
    let dir = project_dir(&s, &id)?;
    let dump = safe_file(&dir, &b.dump)?;
    Ok(stream_task(s, move |state, mut log| async move {
        let d2 = dir.clone();
        let (plan, _diags, project_dir) = tokio::task::spawn_blocking(move || compute(&d2, false)).await.map_err(|e| e.to_string())??;
        let plan = plan.ok_or("plan refusé : corrigez les erreurs")?;
        let resolver = crate::plan::registry::Registry::new()?;
        let plan = crate::plan::initdb::seed_plan(&plan, &b.service, &dump, &project_dir, &resolver, b.local)?;
        let opts = crate::exec::reconcile::Options { project_dir: &project_dir, dry_run: false, no_wait: b.no_wait, confirm_addons: b.yes, assets: false };
        crate::exec::reconcile::up(&plan, &api_of(&state, &org).await.map_err(|e| e.1)?, &opts, &mut log).await.map(|_| ())
    }))
}

/// Reçoit un dump quelconque et le range dans le projet
/// (`.clever-compose/seed/<nom>`), puisque `seed` attend un fichier du
/// projet ; renvoie son chemin relatif.
async fn local_seed_upload(AxState(s): AxState<Shared>, UrlPath(id): UrlPath<String>, mut multipart: Multipart) -> Result<Json<J>, ApiError> {
    let dir = project_dir(&s, &id)?;
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
async fn local_scaling(AxState(s): AxState<Shared>, UrlPath(id): UrlPath<String>, Json(b): Json<ScalingBody>) -> Result<Json<J>, ApiError> {
    let dir = project_dir(&s, &id)?;
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
async fn local_assets_sync(AxState(s): AxState<Shared>, UrlPath(id): UrlPath<String>) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let dir = project_dir(&s, &id)?;
    Ok(stream_task(s, move |_state, mut log| async move {
        let (plan, diags, project_dir) = tokio::task::spawn_blocking(move || compute(&dir, false)).await.map_err(|e| e.to_string())??;
        let plan = plan.filter(|_| !diags.as_array().into_iter().flatten().any(|d| d["severity"] == "error")).ok_or("plan refusé : corrigez les erreurs")?;
        let opts = crate::exec::reconcile::Options { project_dir: &project_dir, dry_run: false, no_wait: true, confirm_addons: false, assets: true };
        crate::exec::assets::sync(&plan, &project_dir, &opts, &mut log).await
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
async fn local_export(AxState(s): AxState<Shared>, UrlPath(id): UrlPath<String>, body: Option<Json<ExportBody>>) -> Result<Json<J>, ApiError> {
    let dir = project_dir(&s, &id)?;
    let b = body.map(|Json(b)| b).unwrap_or_default();
    let result = tokio::task::spawn_blocking(move || -> Result<J, String> {
        let (plan, diags, project_dir) = compute(&dir, b.allow_ephemeral)?;
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
