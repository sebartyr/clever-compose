//! Accès à la plateforme derrière un trait (CLAUDE.md §2) : l'implémentation
//! réelle passe par le client signé du SDK Rust (OAuth 1.0 ou bearer), les
//! routes absentes du SDK étant appelées directement avec ce même client.
//! Les manques constatés sont listés dans summary.md pour des PR au SDK.

use std::future::Future;

use clevercloud_sdk::oauth10a::reqwest;
use clevercloud_sdk::{Client, Credentials, Request, RestClient};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as J};

pub type PResult<T> = Result<T, String>;

#[derive(Debug, Clone, Deserialize)]
pub struct AppInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub zone: String,
    pub deployment: DeploymentInfo,
    #[serde(default)]
    pub state: String,
    pub instance: InstanceInfo,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeploymentInfo {
    #[serde(rename = "httpUrl")]
    pub http_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InstanceInfo {
    #[serde(rename = "minInstances")]
    pub min_instances: u32,
    #[serde(rename = "maxInstances")]
    pub max_instances: u32,
    #[serde(rename = "minFlavor")]
    pub min_flavor: Flavor,
    #[serde(rename = "maxFlavor")]
    pub max_flavor: Flavor,
    #[serde(default)]
    pub lifetime: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Flavor {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EnvVar {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NgInfo {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub members: Vec<NgMember>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NgMember {
    pub id: String,
    #[serde(rename = "domainName", default)]
    pub domain_name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeploymentState {
    pub id: u64,
    #[serde(default)]
    pub uuid: String,
    pub state: String,
    #[serde(default)]
    pub action: String,
    #[serde(default)]
    pub date: u64,
}

/// Une instance en cours (`/instances`).
#[derive(Debug, Clone, Deserialize)]
pub struct InstanceState {
    pub id: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub flavor: Option<Flavor>,
    #[serde(rename = "deployNumber", default)]
    pub deploy_number: u64,
}

/// Cible d'un flux de logs : application ou ressource (addon).
#[derive(Debug, Clone, Copy)]
pub enum LogTarget<'a> {
    App(&'a str),
    Resource(&'a str),
}

/// Ce que demande un flux de logs (bornes RFC 3339, nombre de lignes).
#[derive(Debug, Clone, Default)]
pub struct LogQuery {
    pub since: Option<String>,
    pub until: Option<String>,
    pub limit: Option<usize>,
}

/// Une ligne de logs de la route v4 (`APPLICATION_LOG`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    #[serde(default)]
    pub date: String,
    #[serde(default)]
    pub instance_id: String,
    #[serde(default)]
    pub service: String,
    #[serde(default)]
    pub message: String,
}

/// Un événement du flux : une ligne, ou la fin annoncée par le serveur
/// (`END_OF_STREAM`, `endedBy` = `LIMIT_REACHED` | `UNTIL_REACHED` | `TIMEOUT_REACHED`).
#[derive(Debug, Clone)]
pub enum LogEvent {
    Line(LogLine),
    End(String),
}

/// Flux SSE de logs, lu trame par trame (`event:` / `data:` / ligne vide).
pub struct LogStream {
    resp: reqwest::Response,
    buf: Vec<u8>,
    done: bool,
}

impl LogStream {
    /// Événement suivant ; `None` quand le serveur a fermé la connexion.
    pub async fn next(&mut self) -> PResult<Option<LogEvent>> {
        loop {
            if let Some(ev) = self.take_frame()? {
                return Ok(Some(ev));
            }
            if self.done {
                return Ok(None);
            }
            match self.resp.chunk().await.map_err(|e| format!("flux de logs : {e}"))? {
                Some(bytes) => self.buf.extend_from_slice(&bytes),
                None => self.done = true,
            }
        }
    }

    /// Une trame complète du tampon, si elle y est (terminée par une ligne
    /// vide) ; les commentaires (`:`) et les trames sans événement utile
    /// sont sautés.
    fn take_frame(&mut self) -> PResult<Option<LogEvent>> {
        loop {
            let Some(end) = self.buf.windows(2).position(|w| w == b"\n\n") else { return Ok(None) };
            let frame: Vec<u8> = self.buf.drain(..end + 2).collect();
            let text = String::from_utf8_lossy(&frame);
            let (mut event, mut data) = (String::new(), String::new());
            for line in text.lines() {
                if let Some(v) = line.strip_prefix("event:") {
                    event = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("data:") {
                    if !data.is_empty() {
                        data.push('\n');
                    }
                    data.push_str(v.trim_start());
                }
            }
            match event.as_str() {
                "APPLICATION_LOG" => return serde_json::from_str::<LogLine>(&data).map(|l| Some(LogEvent::Line(l))).map_err(|e| format!("ligne de log illisible : {e} ({data})")),
                "END_OF_STREAM" => {
                    let v: J = serde_json::from_str(&data).unwrap_or(J::Null);
                    return Ok(Some(LogEvent::End(v["endedBy"].as_str().unwrap_or("").to_string())));
                }
                _ => continue,
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Redir {
    pub namespace: String,
    pub port: u16,
}

#[derive(Debug, Clone)]
pub struct NewApp {
    pub name: String,
    pub region: String,
    pub flavor: String,
    pub min_instances: u32,
    pub max_instances: u32,
    pub task: bool,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct AddonInfo {
    pub id: String,
    pub name: String,
    pub provider: String,
    pub plan_slug: String,
}

#[derive(Debug, Clone)]
pub struct NewAddon {
    pub name: String,
    pub provider: String,
    /// Slug du plan ; l'implémentation retrouve son id.
    pub plan: String,
    pub region: String,
    pub version: Option<String>,
}

/// Ce que l'exécuteur demande à la plateforme.
pub trait Platform: Send + Sync {
    fn list_addons(&self) -> impl Future<Output = PResult<Vec<AddonInfo>>> + Send;
    fn create_addon(&self, addon: &NewAddon) -> impl Future<Output = PResult<AddonInfo>> + Send;
    fn linked_addons(&self, app: &str) -> impl Future<Output = PResult<Vec<String>>> + Send;
    fn link_addon(&self, app: &str, addon: &str) -> impl Future<Output = PResult<()>> + Send;
    fn list_apps(&self) -> impl Future<Output = PResult<Vec<AppInfo>>> + Send;
    fn get_app(&self, app: &str) -> impl Future<Output = PResult<AppInfo>> + Send;
    fn create_app(&self, app: &NewApp) -> impl Future<Output = PResult<AppInfo>> + Send;
    fn update_scaling(&self, app: &str, flavor: &str, min: u32, max: u32) -> impl Future<Output = PResult<()>> + Send;
    fn get_tags(&self, app: &str) -> impl Future<Output = PResult<Vec<String>>> + Send;
    fn set_tags(&self, app: &str, tags: &[String]) -> impl Future<Output = PResult<()>> + Send;
    fn get_env(&self, app: &str) -> impl Future<Output = PResult<Vec<EnvVar>>> + Send;
    fn set_env(&self, app: &str, env: &[EnvVar]) -> impl Future<Output = PResult<()>> + Send;
    fn list_ngs(&self) -> impl Future<Output = PResult<Vec<NgInfo>>> + Send;
    fn create_ng(&self, label: &str, tags: &[String]) -> impl Future<Output = PResult<NgInfo>> + Send;
    fn link_member(&self, ng: &str, app: &str) -> impl Future<Output = PResult<()>> + Send;
    fn list_tcp_redirs(&self, app: &str) -> impl Future<Output = PResult<Vec<Redir>>> + Send;
    fn add_tcp_redir(&self, app: &str, namespace: &str) -> impl Future<Output = PResult<Redir>> + Send;
    fn list_udp_redirs(&self, app: &str) -> impl Future<Output = PResult<Vec<Redir>>> + Send;
    fn add_udp_redir(&self, app: &str, namespace: &str) -> impl Future<Output = PResult<Redir>> + Send;
    fn list_domains(&self, app: &str) -> impl Future<Output = PResult<Vec<String>>> + Send;
    fn add_domain(&self, app: &str, fqdn: &str) -> impl Future<Output = PResult<()>> + Send;
    fn redeploy(&self, app: &str) -> impl Future<Output = PResult<()>> + Send;
    fn last_deployment(&self, app: &str) -> impl Future<Output = PResult<Option<DeploymentState>>> + Send;
    fn instances(&self, app: &str) -> impl Future<Output = PResult<Vec<InstanceState>>> + Send;
    /// Variables qu'un addon injecte dans les applications liées.
    fn addon_env(&self, addon: &str) -> impl Future<Output = PResult<Vec<EnvVar>>> + Send;
    /// Ouvre le flux de logs (SSE, route v4) d'une application ou d'un addon.
    fn logs(&self, target: LogTarget<'_>, query: &LogQuery) -> impl Future<Output = PResult<LogStream>> + Send;
    fn delete_app(&self, app: &str) -> impl Future<Output = PResult<()>> + Send;
    fn delete_addon(&self, addon: &str) -> impl Future<Output = PResult<()>> + Send;
    fn delete_ng(&self, ng: &str) -> impl Future<Output = PResult<()>> + Send;
    /// Identifiants pour le push git HTTPS (`user`, `password`).
    fn git_credentials(&self) -> (String, String);
}

/// Identifiants de l'outil, indépendants de la CLI clever : `CC_API_TOKEN`
/// (bearer, api-bridge) ou `CLEVER_TOKEN` + `CLEVER_SECRET` (OAuth 1.0).
pub fn credentials_from_env() -> PResult<Credentials> {
    if let Ok(token) = std::env::var("CC_API_TOKEN") {
        return Ok(Credentials::Bearer { token });
    }
    match (std::env::var("CLEVER_TOKEN"), std::env::var("CLEVER_SECRET")) {
        (Ok(token), Ok(secret)) => Ok(Credentials::OAuth1 {
            token,
            secret,
            consumer_key: std::env::var("CLEVER_CONSUMER_KEY").unwrap_or_else(|_| clevercloud_sdk::DEFAULT_CONSUMER_KEY.into()),
            consumer_secret: std::env::var("CLEVER_CONSUMER_SECRET").unwrap_or_else(|_| clevercloud_sdk::DEFAULT_CONSUMER_SECRET.into()),
        }),
        _ => Err("identifiants absents : CC_API_TOKEN, ou CLEVER_TOKEN et CLEVER_SECRET (jeton OAuth 1.0)".into()),
    }
}

pub struct Api {
    client: Client,
    owner: String,
    creds: Credentials,
    instance: std::sync::OnceLock<J>,
}

impl Api {
    pub fn new(owner: String, creds: Credentials) -> Self {
        let client = Client::builder().with_credentials(creds.clone()).build(clevercloud_sdk::oauth10a::reqwest::Client::new());
        Api { client, owner, creds, instance: std::sync::OnceLock::new() }
    }

    fn base(&self) -> String {
        clevercloud_sdk::PUBLIC_ENDPOINT.to_string()
    }

    fn owner_path(&self) -> String {
        if self.owner.starts_with("user_") { format!("{}/v2/self", self.base()) } else { format!("{}/v2/organisations/{}", self.base(), self.owner) }
    }

    fn app_path(&self, app: &str) -> String {
        format!("{}/applications/{app}", self.owner_path())
    }

    fn e(op: &str, err: impl std::fmt::Display) -> String {
        format!("API {op} : {err}")
    }

    /// Requête signée dont la réponse peut être vide (201 sans corps) ou en
    /// JSON : renvoie le corps brut, erreur si le statut n'est pas 2xx.
    async fn send(&self, method: reqwest::Method, url: &str, body: Option<&J>) -> PResult<String> {
        let mut req = reqwest::Request::new(method, url.parse().map_err(|e| format!("URL {url} : {e}"))?);
        if let Some(b) = body {
            req.headers_mut().insert(reqwest::header::CONTENT_TYPE, "application/json".parse().unwrap());
            *req.body_mut() = Some(serde_json::to_vec(b).map_err(|e| e.to_string())?.into());
        }
        req.headers_mut().insert(reqwest::header::ACCEPT, "application/json".parse().unwrap());
        let resp = self.client.execute(req).await.map_err(|e| Self::e(url, e))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!("API {url} : {status} {text}"));
        }
        Ok(text)
    }

    /// Le type d'instance Docker courant (version, variante), lu sur l'API.
    async fn docker_instance(&self) -> PResult<J> {
        if let Some(i) = self.instance.get() {
            return Ok(i.clone());
        }
        let all: Vec<J> = self.client.get(&format!("{}/v2/products/instances", self.base())).await.map_err(|e| Self::e("products/instances", e))?;
        let docker = all.into_iter().find(|i| i["type"] == "docker").ok_or("type d'instance docker introuvable")?;
        let _ = self.instance.set(docker.clone());
        Ok(docker)
    }
}

impl Platform for Api {
    async fn list_addons(&self) -> PResult<Vec<AddonInfo>> {
        let all = clevercloud_sdk::v2::addon::list(&self.client, &self.owner).await.map_err(|e| e.to_string())?;
        Ok(all.into_iter().map(|a| AddonInfo { id: a.id, name: a.name.unwrap_or_default(), provider: a.provider.id, plan_slug: a.plan.slug }).collect())
    }

    async fn create_addon(&self, addon: &NewAddon) -> PResult<AddonInfo> {
        // L'id du plan vient de la liste des fournisseurs (relue, jamais
        // codée). `v2::plan::list` du SDK échoue sur un champ null de cette
        // route (à corriger dans le SDK) : lecture en JSON libre.
        let providers: Vec<J> = self.client.get(&format!("{}/v2/products/addonproviders", self.base())).await.map_err(|e| Self::e("products/addonproviders", e))?;
        let provider = providers.iter().find(|p| p["id"] == addon.provider.as_str()).ok_or_else(|| format!("fournisseur `{}` inconnu", addon.provider))?;
        let plan_id = provider["plans"].as_array().and_then(|ps| ps.iter().find(|p| p["slug"] == addon.plan.as_str())).and_then(|p| p["id"].as_str()).ok_or_else(|| format!("plan `{}` inconnu pour {}", addon.plan, addon.provider))?;
        let opts = clevercloud_sdk::v2::addon::CreateOpts {
            name: addon.name.clone(),
            region: addon.region.clone(),
            provider_id: addon.provider.clone(),
            plan: plan_id.to_string(),
            options: clevercloud_sdk::v2::addon::Opts { version: addon.version.clone(), encryption: None, services: None },
        };
        let a = clevercloud_sdk::v2::addon::create(&self.client, &self.owner, &opts).await.map_err(|e| e.to_string())?;
        Ok(AddonInfo { id: a.id, name: a.name.unwrap_or_default(), provider: a.provider.id, plan_slug: a.plan.slug })
    }

    async fn linked_addons(&self, app: &str) -> PResult<Vec<String>> {
        let v: Vec<J> = self.client.get(&format!("{}/addons", self.app_path(app))).await.map_err(|e| Self::e("addons liés", e))?;
        Ok(v.into_iter().filter_map(|a| a["id"].as_str().map(str::to_string)).collect())
    }

    async fn link_addon(&self, app: &str, addon: &str) -> PResult<()> {
        // Corps : l'id de l'addon, en chaîne JSON (clever-client `linkAddon`).
        self.send(reqwest::Method::POST, &format!("{}/addons", self.app_path(app)), Some(&J::String(addon.to_string()))).await?;
        Ok(())
    }

    async fn list_apps(&self) -> PResult<Vec<AppInfo>> {
        self.client.get(&format!("{}/applications", self.owner_path())).await.map_err(|e| Self::e("applications", e))
    }

    async fn get_app(&self, app: &str) -> PResult<AppInfo> {
        self.client.get(&self.app_path(app)).await.map_err(|e| Self::e("application", e))
    }

    async fn create_app(&self, app: &NewApp) -> PResult<AppInfo> {
        let inst = self.docker_instance().await?;
        let body = json!({
            "name": app.name,
            "description": app.description,
            "deploy": "git",
            "instanceType": "docker",
            "instanceVersion": inst["version"],
            "instanceVariant": inst["variant"]["id"],
            "minFlavor": app.flavor,
            "maxFlavor": app.flavor,
            "minInstances": app.min_instances,
            "maxInstances": app.max_instances,
            "zone": app.region,
            "instanceLifetime": if app.task { "TASK" } else { "REGULAR" },
            "shutdownable": false,
            "cancelOnPush": true,
            "separateBuild": false,
        });
        self.client.post(&format!("{}/applications", self.owner_path()), &body).await.map_err(|e| Self::e("création d'application", e))
    }

    async fn update_scaling(&self, app: &str, flavor: &str, min: u32, max: u32) -> PResult<()> {
        let body = json!({ "minFlavor": flavor, "maxFlavor": flavor, "minInstances": min, "maxInstances": max });
        self.send(reqwest::Method::PUT, &self.app_path(app), Some(&body)).await?;
        Ok(())
    }

    async fn get_tags(&self, app: &str) -> PResult<Vec<String>> {
        self.client.get(&format!("{}/tags", self.app_path(app))).await.map_err(|e| Self::e("tags", e))
    }

    async fn set_tags(&self, app: &str, tags: &[String]) -> PResult<()> {
        self.send(reqwest::Method::PUT, &format!("{}/tags", self.app_path(app)), Some(&json!(tags))).await?;
        Ok(())
    }

    async fn get_env(&self, app: &str) -> PResult<Vec<EnvVar>> {
        self.client.get(&format!("{}/env", self.app_path(app))).await.map_err(|e| Self::e("env", e))
    }

    async fn set_env(&self, app: &str, env: &[EnvVar]) -> PResult<()> {
        // PUT remplace tout l'environnement (objet nom → valeur, alors que le
        // GET renvoie une liste) : c'est ce que la réconciliation veut, ce que
        // le plan ne pose plus disparaît.
        let body: serde_json::Map<String, J> = env.iter().map(|e| (e.name.clone(), J::String(e.value.clone()))).collect();
        self.send(reqwest::Method::PUT, &format!("{}/env", self.app_path(app)), Some(&J::Object(body))).await?;
        Ok(())
    }

    async fn list_ngs(&self) -> PResult<Vec<NgInfo>> {
        self.client.get(&format!("{}/v4/networkgroups/organisations/{}/networkgroups", self.base(), self.owner)).await.map_err(|e| Self::e("network groups", e))
    }

    async fn create_ng(&self, label: &str, tags: &[String]) -> PResult<NgInfo> {
        let url = format!("{}/v4/networkgroups/organisations/{}/networkgroups", self.base(), self.owner);
        let body = json!({ "ownerId": self.owner, "label": label, "description": format!("clever-compose : {label}"), "tags": tags });
        // La création répond sans corps : on relit la liste.
        self.send(reqwest::Method::POST, &url, Some(&body)).await?;
        let all = self.list_ngs().await?;
        all.into_iter().find(|n| n.label == label).ok_or_else(|| "network group créé mais introuvable".to_string())
    }

    async fn link_member(&self, ng: &str, app: &str) -> PResult<()> {
        let url = format!("{}/v4/networkgroups/organisations/{}/networkgroups/{ng}/members", self.base(), self.owner);
        let body = json!({ "id": app, "kind": "application", "label": app });
        self.send(reqwest::Method::POST, &url, Some(&body)).await?;
        Ok(())
    }

    async fn list_tcp_redirs(&self, app: &str) -> PResult<Vec<Redir>> {
        self.client.get(&format!("{}/tcpRedirs", self.app_path(app))).await.map_err(|e| Self::e("tcpRedirs", e))
    }

    async fn add_tcp_redir(&self, app: &str, namespace: &str) -> PResult<Redir> {
        self.client.post(&format!("{}/tcpRedirs", self.app_path(app)), &json!({ "namespace": namespace })).await.map_err(|e| Self::e("tcpRedirs", e))
    }

    async fn list_udp_redirs(&self, app: &str) -> PResult<Vec<Redir>> {
        self.client.get(&format!("{}/udpRedirs", self.app_path(app))).await.map_err(|e| Self::e("udpRedirs", e))
    }

    async fn add_udp_redir(&self, app: &str, namespace: &str) -> PResult<Redir> {
        self.client.post(&format!("{}/udpRedirs", self.app_path(app)), &json!({ "namespace": namespace })).await.map_err(|e| Self::e("udpRedirs", e))
    }

    async fn list_domains(&self, app: &str) -> PResult<Vec<String>> {
        let v: Vec<J> = self.client.get(&format!("{}/vhosts", self.app_path(app))).await.map_err(|e| Self::e("vhosts", e))?;
        Ok(v.into_iter().filter_map(|x| x["fqdn"].as_str().map(str::to_string)).collect())
    }

    async fn add_domain(&self, app: &str, fqdn: &str) -> PResult<()> {
        self.send(reqwest::Method::PUT, &format!("{}/vhosts/{fqdn}", self.app_path(app)), None).await?;
        Ok(())
    }

    async fn redeploy(&self, app: &str) -> PResult<()> {
        self.send(reqwest::Method::POST, &format!("{}/instances", self.app_path(app)), None).await?;
        Ok(())
    }

    async fn last_deployment(&self, app: &str) -> PResult<Option<DeploymentState>> {
        let v: Vec<DeploymentState> = self.client.get(&format!("{}/deployments?limit=1", self.app_path(app))).await.map_err(|e| Self::e("deployments", e))?;
        Ok(v.into_iter().next())
    }

    async fn addon_env(&self, addon: &str) -> PResult<Vec<EnvVar>> {
        self.client.get(&format!("{}/addons/{addon}/env", self.owner_path())).await.map_err(|e| Self::e("addon env", e))
    }

    async fn instances(&self, app: &str) -> PResult<Vec<InstanceState>> {
        self.client.get(&format!("{}/instances", self.app_path(app))).await.map_err(|e| Self::e("instances", e))
    }

    async fn logs(&self, target: LogTarget<'_>, query: &LogQuery) -> PResult<LogStream> {
        let path = match target {
            LogTarget::App(id) => format!("{}/v4/logs/organisations/{}/applications/{id}/logs", self.base(), self.owner),
            LogTarget::Resource(id) => format!("{}/v4/logs/organisations/{}/resources/{id}/logs", self.base(), self.owner),
        };
        let mut url: reqwest::Url = path.parse().map_err(|e| format!("URL {path} : {e}"))?;
        {
            let mut q = url.query_pairs_mut();
            if let Some(s) = &query.since {
                q.append_pair("since", s);
            }
            if let Some(u) = &query.until {
                q.append_pair("until", u);
            }
            if let Some(l) = query.limit {
                q.append_pair("limit", &l.to_string());
            }
        }
        let mut req = reqwest::Request::new(reqwest::Method::GET, url);
        req.headers_mut().insert(reqwest::header::ACCEPT, "text/event-stream".parse().unwrap());
        let resp = self.client.execute(req).await.map_err(|e| Self::e("logs", e))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("API logs : {status} {text}"));
        }
        Ok(LogStream { resp, buf: Vec::new(), done: false })
    }

    async fn delete_app(&self, app: &str) -> PResult<()> {
        self.send(reqwest::Method::DELETE, &self.app_path(app), None).await?;
        Ok(())
    }

    async fn delete_addon(&self, addon: &str) -> PResult<()> {
        clevercloud_sdk::v2::addon::delete(&self.client, &self.owner, addon).await.map_err(|e| e.to_string())
    }

    async fn delete_ng(&self, ng: &str) -> PResult<()> {
        self.send(reqwest::Method::DELETE, &format!("{}/v4/networkgroups/organisations/{}/networkgroups/{ng}", self.base(), self.owner), None).await?;
        Ok(())
    }

    fn git_credentials(&self) -> (String, String) {
        match &self.creds {
            Credentials::OAuth1 { token, secret, .. } => (token.clone(), secret.clone()),
            Credentials::Bearer { token } => ("token".into(), token.clone()),
            Credentials::Basic { username, password } => (username.clone(), password.clone()),
        }
    }
}
