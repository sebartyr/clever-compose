//! Identifiants du poste et connexion (`clever-compose login`). Danse
//! OAuth 1.0 à trois pattes avec le consumer de clever-compose (embarqué,
//! base URL `http://127.0.0.1:8080/`), ou celui de l'environnement
//! (`CLEVER_CONSUMER_KEY` / `CLEVER_CONSUMER_SECRET`, qui prime toujours) :
//! request token avec un callback local (`http://127.0.0.1:<port>/callback`),
//! autorisation dans le navigateur, access token. Les jetons sont gardés
//! dans `$XDG_CONFIG_HOME/clever-cloud/clever-compose.json` (notre
//! fichier, à côté de celui de clever-tools, jamais dedans).
//!
//! Ordre de résolution des identifiants : `CLEVER_COMPOSE_API_TOKEN`, puis
//! `CLEVER_TOKEN` + `CLEVER_SECRET` (signés avec le consumer courant),
//! puis le fichier.

use std::path::PathBuf;

use clevercloud_sdk::Credentials;
use serde::{Deserialize, Serialize};

use crate::exec::platform::Api;

/// Consumer OAuth 1.0 de l'utilisateur.
#[derive(Clone)]
pub struct Consumer {
    pub key: String,
    pub secret: String,
}

/// Consumer OAuth « clever-compose » (console Clever Cloud). Comme
/// clever-tools embarque le sien : un consumer public identifie
/// l'application, il ne donne accès à rien sans l'autorisation de
/// l'utilisateur. Clever Cloud n'accepte un `oauth_callback` que sous la
/// base URL déclarée pour le consumer : celle-ci décide donc d'où la
/// danse peut se faire (le poste, ou une application qui héberge
/// `ui`/`api`) ; `CLEVER_CONSUMER_KEY`/`SECRET` pour en utiliser un autre.
pub const DEFAULT_CONSUMER_KEY: &str = "4cUX75GQHaKwJIBnjj63eXrKNiWlqe";
/// Vérifié le 17/09/2026 : request token accepté avec ces constantes.
pub const DEFAULT_CONSUMER_SECRET: &str = "x1pMMlJBXy9u5QL1YjvTEXGe0IK4xh";

pub const CONSUMER_HINT: &str = "CLEVER_CONSUMER_KEY et CLEVER_CONSUMER_SECRET requis dans l'environnement (consumer OAuth créé dans la console Clever Cloud) : aucun consumer par défaut dans cette compilation";

impl Consumer {
    /// L'environnement d'abord, sinon le consumer embarqué.
    pub fn from_env() -> Result<Self, String> {
        match (std::env::var("CLEVER_CONSUMER_KEY"), std::env::var("CLEVER_CONSUMER_SECRET")) {
            (Ok(key), Ok(secret)) if !key.is_empty() && !secret.is_empty() => Ok(Consumer { key, secret }),
            _ if !DEFAULT_CONSUMER_SECRET.is_empty() => Ok(Consumer { key: DEFAULT_CONSUMER_KEY.into(), secret: DEFAULT_CONSUMER_SECRET.into() }),
            _ => Err(CONSUMER_HINT.into()),
        }
    }
}

/// Jetons gardés sur le poste.
#[derive(Serialize, Deserialize, Clone)]
pub struct Stored {
    pub token: String,
    pub secret: String,
    /// Clé du consumer qui a émis ces jetons : ils ne valent qu'avec lui.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consumer_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// `~/.config/clever-cloud/clever-compose.json`.
pub fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join(".config"));
    base.join("clever-cloud").join("clever-compose.json")
}

/// Un échec qui ressemble à des identifiants absents, invalides ou expirés.
pub fn looks_like_auth_failure(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("identifiants absents") || m.contains("invalid-token") || m.contains("invalid token") || m.contains("unknown token") || m.contains("jetons refusés") || m.contains("401 unauthorized") || m.contains("aucun identifiant")
}

pub fn load() -> Option<Stored> {
    let text = std::fs::read_to_string(config_path()).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save(s: &Stored) -> Result<PathBuf, String> {
    let path = config_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{} : {e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    // Créé en 0600 dès l'ouverture (jamais lisible par d'autres, même un
    // instant), puis mode imposé si le fichier existait déjà.
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    {
        use std::io::Write;
        let mut f = opts.open(&path).map_err(|e| format!("{} : {e}", path.display()))?;
        f.write_all(text.as_bytes()).map_err(|e| format!("{} : {e}", path.display()))?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(path)
}

pub fn remove() -> Result<bool, String> {
    let path = config_path();
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("{} : {e}", path.display())),
    }
}

/// D'où viennent les identifiants retenus.
pub enum Source {
    Env,
    File(PathBuf),
}

/// Identifiants du poste : environnement, sinon fichier de `login`.
pub fn credentials() -> Result<(Credentials, Source), String> {
    if let Ok(token) = std::env::var("CLEVER_COMPOSE_API_TOKEN") {
        if !token.is_empty() {
            return Ok((Credentials::Bearer { token }, Source::Env));
        }
    }
    if let (Ok(token), Ok(secret)) = (std::env::var("CLEVER_TOKEN"), std::env::var("CLEVER_SECRET")) {
        let consumer = Consumer::from_env()?;
        return Ok((Credentials::OAuth1 { token, secret, consumer_key: consumer.key, consumer_secret: consumer.secret }, Source::Env));
    }
    if let Some(s) = load() {
        let consumer = Consumer::from_env()?;
        if s.consumer_key.as_deref().is_some_and(|k| k != consumer.key) {
            return Err(format!("les jetons de {} viennent d'un autre consumer que CLEVER_CONSUMER_KEY : `clever-compose login` avec celui-ci", config_path().display()));
        }
        return Ok((Credentials::OAuth1 { token: s.token, secret: s.secret, consumer_key: consumer.key, consumer_secret: consumer.secret }, Source::File(config_path())));
    }
    Err("identifiants absents : `clever-compose login`, ou CLEVER_COMPOSE_API_TOKEN, ou CLEVER_TOKEN et CLEVER_SECRET dans l'environnement".into())
}

// ------------------------------------------------------------ danse OAuth 1.0

/// Encodage percent d'OAuth (RFC 3986, non réservés seulement).
pub fn oauth_enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

fn oauth_nonce() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| format!("aléa : {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Paramètres OAuth communs, signature `PLAINTEXT` (le transport est TLS ;
/// la doc de l'API l'admet).
fn oauth_params(consumer: &Consumer, token_secret: &str) -> Result<Vec<(String, String)>, String> {
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    Ok(vec![
        ("oauth_consumer_key".into(), consumer.key.clone()),
        ("oauth_signature_method".into(), "PLAINTEXT".into()),
        ("oauth_signature".into(), format!("{}&{}", oauth_enc(&consumer.secret), oauth_enc(token_secret))),
        ("oauth_timestamp".into(), ts.to_string()),
        ("oauth_nonce".into(), oauth_nonce()?),
        ("oauth_version".into(), "1.0".into()),
    ])
}

pub fn parse_form(text: &str) -> std::collections::HashMap<String, String> {
    text.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.replace('+', " "))).collect()
}

async fn oauth_post(url: &str, what: &str) -> Result<std::collections::HashMap<String, String>, String> {
    let resp = reqwest::Client::new().post(url).send().await.map_err(|e| format!("{what} : {e}"))?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("{what} : {status} {text}"));
    }
    let form = parse_form(&text);
    if !(form.contains_key("oauth_token") && form.contains_key("oauth_token_secret")) {
        return Err(format!("{what} : réponse inattendue : {text}"));
    }
    Ok(form)
}

/// Étape 1 : request token pour `callback`, et l'URL d'autorisation à
/// ouvrir. Rend `(token, secret, url)`.
pub async fn request_token(consumer: &Consumer, callback: &str) -> Result<(String, String, String), String> {
    let mut params = oauth_params(consumer, "")?;
    params.push(("oauth_callback".into(), callback.to_string()));
    let qs: String = params.iter().map(|(k, v)| format!("{k}={}", oauth_enc(v))).collect::<Vec<_>>().join("&");
    let form = oauth_post(&format!("{}/v2/oauth/request_token_query?{qs}", clevercloud_sdk::PUBLIC_ENDPOINT), "request_token").await?;
    let token = form["oauth_token"].clone();
    let url = format!("{}/v2/oauth/authorize?oauth_token={}", clevercloud_sdk::PUBLIC_ENDPOINT, oauth_enc(&token));
    Ok((token, form["oauth_token_secret"].clone(), url))
}

/// Étape 3 : access token à partir du request token autorisé.
pub async fn access_token(consumer: &Consumer, token: &str, request_secret: &str, verifier: &str) -> Result<(String, String), String> {
    let mut params = oauth_params(consumer, request_secret)?;
    params.push(("oauth_token".into(), token.to_string()));
    params.push(("oauth_verifier".into(), verifier.to_string()));
    let qs: String = params.iter().map(|(k, v)| format!("{k}={}", oauth_enc(v))).collect::<Vec<_>>().join("&");
    let form = oauth_post(&format!("{}/v2/oauth/access_token_query?{qs}", clevercloud_sdk::PUBLIC_ENDPOINT), "access_token").await?;
    Ok((form["oauth_token"].clone(), form["oauth_token_secret"].clone()))
}

/// Vérifie des jetons auprès de l'API et complète l'utilisateur.
pub async fn verify(stored: &mut Stored, consumer: &Consumer) -> Result<Api, String> {
    let creds = Credentials::OAuth1 { token: stored.token.clone(), secret: stored.secret.clone(), consumer_key: consumer.key.clone(), consumer_secret: consumer.secret.clone() };
    let api = Api::new(String::new(), creds);
    let me = api.whoami().await.map_err(|e| format!("jetons obtenus mais refusés par l'API : {e}"))?;
    stored.user_id = me.0;
    stored.email = me.1;
    stored.consumer_key = Some(consumer.key.clone());
    Ok(api)
}

/// La danse complète depuis un terminal : callback sur
/// `127.0.0.1:<port>`, URL d'autorisation affichée (et ouverte), attente
/// du retour du navigateur, access token, vérification, enregistrement.
pub async fn login(port: u16, open_browser: bool) -> Result<Stored, String> {
    let consumer = Consumer::from_env()?;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.map_err(|e| format!("écoute sur 127.0.0.1:{port} pour le callback : {e} (--callback-port pour changer, la base URL du consumer doit y correspondre)"))?;
    let callback = format!("http://127.0.0.1:{port}/callback");
    let (token, request_secret, url) = request_token(&consumer, &callback).await?;
    eprintln!("Ouvrez cette page pour autoriser clever-compose :\n  {url}");
    if open_browser {
        let _ = std::process::Command::new("xdg-open").arg(&url).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
    }
    let verifier = tokio::time::timeout(std::time::Duration::from_secs(300), wait_callback(&listener, &token)).await.map_err(|_| "aucun retour du navigateur en 5 minutes : recommencez".to_string())??;
    let (t, s) = access_token(&consumer, &token, &request_secret, &verifier).await?;
    let mut stored = Stored { token: t, secret: s, consumer_key: None, user_id: None, email: None };
    verify(&mut stored, &consumer).await?;
    save(&stored)?;
    Ok(stored)
}

/// Une requête HTTP minimale sur le callback : `GET /callback?oauth_token=…&oauth_verifier=…`.
async fn wait_callback(listener: &tokio::net::TcpListener, expected_token: &str) -> Result<String, String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    loop {
        let (mut sock, _) = listener.accept().await.map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 8192];
        let n = sock.read(&mut buf).await.unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]);
        let target = req.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("");
        let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
        let form: std::collections::HashMap<String, String> = parse_form(query).into_iter().map(|(k, v)| (k, percent_decode(&v))).collect();
        let ok = target.starts_with("/callback") && form.get("oauth_token").is_some_and(|t| t == expected_token) && form.contains_key("oauth_verifier");
        let body = if ok { "<!doctype html><meta charset=utf-8><title>clever-compose</title><p>Accès autorisé : vous pouvez fermer cet onglet et revenir au terminal." } else { "<!doctype html><meta charset=utf-8><title>clever-compose</title><p>Requête inattendue." };
        let _ = sock.write_all(format!("HTTP/1.1 {} \r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", if ok { "200 OK" } else { "404 Not Found" }, body.len()).as_bytes()).await;
        let _ = sock.shutdown().await;
        if ok {
            return Ok(form["oauth_verifier"].clone());
        }
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}
