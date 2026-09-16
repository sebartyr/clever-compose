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

pub fn router(state: Shared) -> Router {
    Router::new()
        .route("/api/info", get(info))
        .route("/api/session", get(session_get).post(session_set))
        .route("/api/session/oauth/start", get(oauth_start))
        .route("/api/session/oauth/callback", get(oauth_callback))
        .route("/api/orgs", get(orgs))
        .route("/api/platform/projects", get(platform_projects))
        .route("/api/platform/projects/{project}/inventory", get(inventory))
        .route("/api/platform/projects/{project}/logs", get(logs))
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
    let org = s.session.read().unwrap().as_ref().map(|x| x.org.clone()).unwrap_or_default();
    let stamp = env!("CC_BUILD_STAMP");
    let (commit, secs) = stamp.split_once(' ').unwrap_or((stamp, "0"));
    Json(json!({ "org": org, "version": env!("CARGO_PKG_VERSION"), "commit": commit, "built": crate::exec::inspect::rfc3339(secs.parse().unwrap_or(0)), "bundled": cfg!(feature = "ui-bundled"), "assets_bucket": std::env::var(crate::exec::assets::ENV_BUCKET).ok() }))
}

/// Session courante : identifiants posés ?, organisation choisie, danse
/// OAuth disponible ?
async fn session_get(AxState(s): AxState<Shared>) -> Json<J> {
    let guard = s.session.read().unwrap();
    Json(json!({ "authenticated": guard.is_some(), "org": guard.as_ref().map(|x| x.org.clone()).unwrap_or_default(), "oauth": s.oauth.is_some() }))
}

#[derive(Deserialize, Default)]
struct SessionBody {
    org: Option<String>,
}

/// Choisit l'organisation de la session (les identifiants viennent de
/// l'environnement ou de la danse OAuth, jamais du corps).
async fn session_set(AxState(s): AxState<Shared>, Json(b): Json<SessionBody>) -> Result<Json<J>, ApiError> {
    let api = s.api().map_err(|e| ApiError(StatusCode::UNAUTHORIZED, e))?;
    let orgs = api.organisations().await.map_err(|e| ApiError(StatusCode::UNAUTHORIZED, format!("identifiants refusés : {e}")))?;
    let org = b.org.filter(|o| !o.is_empty()).or_else(|| s.session.read().unwrap().as_ref().map(|x| x.org.clone()).filter(|o| !o.is_empty())).unwrap_or_default();
    let api = api.for_owner(&org);
    *s.session.write().unwrap() = Some(super::Session { api, org: org.clone() });
    Ok(Json(json!({ "authenticated": true, "org": org, "orgs": orgs })))
}

/// Encodage percent d'OAuth (RFC 3986, non réservés seulement).
fn oauth_enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn oauth_nonce() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
    format!("{:x}{:x}{:x}", t.as_nanos(), std::process::id(), rand_u64())
}

fn rand_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new().build_hasher().finish()
}

/// Paramètres OAuth communs, signature `PLAINTEXT` (le transport est TLS ;
/// la doc de l'API l'admet).
fn oauth_params(consumer: &super::OauthConsumer, token_secret: &str) -> Vec<(String, String)> {
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    vec![
        ("oauth_consumer_key".into(), consumer.key.clone()),
        ("oauth_signature_method".into(), "PLAINTEXT".into()),
        ("oauth_signature".into(), format!("{}&{}", oauth_enc(&consumer.secret), oauth_enc(token_secret))),
        ("oauth_timestamp".into(), ts.to_string()),
        ("oauth_nonce".into(), oauth_nonce()),
        ("oauth_version".into(), "1.0".into()),
    ]
}

fn parse_form(text: &str) -> std::collections::HashMap<String, String> {
    text.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.replace('+', " "))).collect()
}

/// Étape 1 de la danse : request token, puis redirection vers la page
/// d'autorisation de Clever Cloud.
async fn oauth_start(AxState(s): AxState<Shared>) -> Result<Response, ApiError> {
    let consumer = s.oauth.clone().ok_or_else(|| bad("pas de consumer OAuth configuré (CLEVER_CONSUMER_KEY / CLEVER_CONSUMER_SECRET)"))?;
    let mut params = oauth_params(&consumer, "");
    params.push(("oauth_callback".into(), format!("{}/api/session/oauth/callback", s.public_base)));
    let qs: String = params.iter().map(|(k, v)| format!("{k}={}", oauth_enc(v))).collect::<Vec<_>>().join("&");
    let url = format!("{}/v2/oauth/request_token_query?{qs}", clevercloud_sdk::PUBLIC_ENDPOINT);
    let resp = clevercloud_sdk::oauth10a::reqwest::Client::new().post(&url).send().await.map_err(|e| internal(format!("request_token : {e}")))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(internal(format!("request_token : {status} {text}")));
    }
    let form = parse_form(&text);
    let (Some(token), Some(secret)) = (form.get("oauth_token"), form.get("oauth_token_secret")) else { return Err(internal(format!("request_token : réponse inattendue : {text}"))) };
    s.pending.lock().unwrap().insert(token.clone(), secret.clone());
    Ok(axum::response::Redirect::temporary(&format!("{}/v2/oauth/authorize?oauth_token={}", clevercloud_sdk::PUBLIC_ENDPOINT, oauth_enc(token))).into_response())
}

#[derive(Deserialize)]
struct OauthCallback {
    oauth_token: String,
    oauth_verifier: String,
}

/// Étape 3 : access token, session ouverte, retour à l'interface.
async fn oauth_callback(AxState(s): AxState<Shared>, Query(q): Query<OauthCallback>) -> Result<Response, ApiError> {
    let consumer = s.oauth.clone().ok_or_else(|| bad("pas de consumer OAuth configuré"))?;
    let request_secret = s.pending.lock().unwrap().remove(&q.oauth_token).ok_or_else(|| bad("request token inconnu ou déjà utilisé : recommencez la connexion"))?;
    let mut params = oauth_params(&consumer, &request_secret);
    params.push(("oauth_token".into(), q.oauth_token.clone()));
    params.push(("oauth_verifier".into(), q.oauth_verifier.clone()));
    let qs: String = params.iter().map(|(k, v)| format!("{k}={}", oauth_enc(v))).collect::<Vec<_>>().join("&");
    let url = format!("{}/v2/oauth/access_token_query?{qs}", clevercloud_sdk::PUBLIC_ENDPOINT);
    let resp = clevercloud_sdk::oauth10a::reqwest::Client::new().post(&url).send().await.map_err(|e| internal(format!("access_token : {e}")))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(internal(format!("access_token : {status} {text}")));
    }
    let form = parse_form(&text);
    let (Some(token), Some(secret)) = (form.get("oauth_token"), form.get("oauth_token_secret")) else { return Err(internal(format!("access_token : réponse inattendue : {text}"))) };
    let creds = clevercloud_sdk::Credentials::OAuth1 { token: token.clone(), secret: secret.clone(), consumer_key: consumer.key.clone(), consumer_secret: consumer.secret.clone() };
    let api = crate::exec::platform::Api::new(String::new(), creds);
    api.organisations().await.map_err(|e| ApiError(StatusCode::UNAUTHORIZED, format!("jeton obtenu mais refusé par l'API : {e}")))?;
    *s.session.write().unwrap() = Some(super::Session { api, org: String::new() });
    Ok(axum::response::Redirect::temporary("/").into_response())
}

async fn orgs(AxState(s): AxState<Shared>) -> Result<Json<J>, ApiError> {
    let api = s.api().map_err(|e| ApiError(StatusCode::UNAUTHORIZED, e))?;
    let orgs = api.organisations().await.map_err(|e| ApiError(StatusCode::UNAUTHORIZED, e))?;
    Ok(Json(json!({ "orgs": orgs })))
}

fn api_of(s: &State) -> Result<crate::exec::platform::Api, ApiError> {
    let api = s.api().map_err(|e| ApiError(StatusCode::UNAUTHORIZED, e))?;
    if api.owner().is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "aucune organisation choisie".into()));
    }
    Ok(api)
}

// ---------------------------------------------------------------- plateforme

async fn platform_projects(AxState(s): AxState<Shared>) -> Result<Json<J>, ApiError> {
    let api = api_of(&s)?;
    let projects = crate::exec::inspect::projects(&api).await.map_err(internal)?;
    Ok(Json(json!({ "projects": projects })))
}

/// L'état d'un projet sur la plateforme, dans une forme prête pour le
/// diagramme : applications (instances, dernier déploiement, domaines),
/// addons, network groups et leurs membres.
async fn inventory(AxState(s): AxState<Shared>, UrlPath(project): UrlPath<String>) -> Result<Json<J>, ApiError> {
    let api = api_of(&s)?;
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

async fn logs(AxState(s): AxState<Shared>, UrlPath(project): UrlPath<String>, Query(q): Query<LogsQuery>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let opts = crate::exec::inspect::LogsOptions { services: q.services.map(|v| v.split(',').filter(|x| !x.is_empty()).map(str::to_string).collect()).unwrap_or_default(), since: q.since, follow: q.follow.unwrap_or(false), limit: q.limit };
    stream_task(s, move |state, mut log| async move { crate::exec::inspect::logs(&project, &api_of(&state).map_err(|e| e.1)?, &opts, &mut log).await })
}

#[derive(Deserialize, Default)]
struct DownBody {
    #[serde(default)]
    yes: bool,
    #[serde(default)]
    dry_run: bool,
}

async fn down(AxState(s): AxState<Shared>, UrlPath(project): UrlPath<String>, body: Option<Json<DownBody>>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let b = body.map(|Json(b)| b).unwrap_or_default();
    stream_task(s, move |state, mut log| async move { crate::exec::reconcile::down(&project, &api_of(&state).map_err(|e| e.1)?, b.dry_run, b.yes, &mut log).await })
}

async fn stop(AxState(s): AxState<Shared>, UrlPath(project): UrlPath<String>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    stream_task(s, move |state, mut log| async move { crate::exec::reconcile::stop_start(&project, &api_of(&state).map_err(|e| e.1)?, false, &mut log).await })
}

async fn start(AxState(s): AxState<Shared>, UrlPath(project): UrlPath<String>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    stream_task(s, move |state, mut log| async move { crate::exec::reconcile::stop_start(&project, &api_of(&state).map_err(|e| e.1)?, true, &mut log).await })
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
    out.sort_by_key(|f| (!f.to_ascii_lowercase().contains("compose"), f.clone()));
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

async fn local_up(AxState(s): AxState<Shared>, UrlPath(id): UrlPath<String>, body: Option<Json<UpBody>>) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let dir = project_dir(&s, &id)?;
    let b = body.map(|Json(b)| b).unwrap_or_default();
    Ok(stream_task(s, move |state, mut log| async move {
        let (plan, diags, project_dir) = tokio::task::spawn_blocking(move || compute(&dir, b.allow_ephemeral)).await.map_err(|e| e.to_string())??;
        for d in diags.as_array().into_iter().flatten() {
            log(&format!("{}{}", if d["severity"] == "error" { "erreur : " } else { "" }, d["rendered"].as_str().unwrap_or("")));
        }
        let plan = plan.filter(|_| !diags.as_array().into_iter().flatten().any(|d| d["severity"] == "error")).ok_or("plan refusé : corrigez les erreurs")?;
        let opts = crate::exec::reconcile::Options { project_dir: &project_dir, dry_run: b.dry_run, no_wait: b.no_wait, confirm_addons: b.yes, assets: b.assets };
        crate::exec::reconcile::up(&plan, &api_of(&state).map_err(|e| e.1)?, &opts, &mut log).await.map(|_| ())
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

async fn local_seed(AxState(s): AxState<Shared>, UrlPath(id): UrlPath<String>, Json(b): Json<SeedBody>) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, ApiError> {
    let dir = project_dir(&s, &id)?;
    let dump = safe_file(&dir, &b.dump)?;
    Ok(stream_task(s, move |state, mut log| async move {
        let d2 = dir.clone();
        let (plan, _diags, project_dir) = tokio::task::spawn_blocking(move || compute(&d2, false)).await.map_err(|e| e.to_string())??;
        let plan = plan.ok_or("plan refusé : corrigez les erreurs")?;
        let resolver = crate::plan::registry::Registry::new()?;
        let plan = crate::plan::initdb::seed_plan(&plan, &b.service, &dump, &project_dir, &resolver, b.local)?;
        let opts = crate::exec::reconcile::Options { project_dir: &project_dir, dry_run: false, no_wait: b.no_wait, confirm_addons: b.yes, assets: false };
        crate::exec::reconcile::up(&plan, &api_of(&state).map_err(|e| e.1)?, &opts, &mut log).await.map(|_| ())
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
