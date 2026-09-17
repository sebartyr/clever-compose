//! Identifiants du poste et connexion (`clever-compose login`), sur le
//! modèle de clever-tools : la page `cli-oauth` de la console fait la
//! danse OAuth 1.0 avec le consumer, puis dépose les jetons que la CLI
//! récupère sur `/v2/self/cli_tokens` (404 tant que l'utilisateur n'a pas
//! autorisé). Les jetons sont gardés dans le fichier de configuration de
//! l'outil (`$XDG_CONFIG_HOME/clever-cloud/clever-compose.json`, à côté
//! de celui de clever-tools, jamais dedans).
//!
//! Ordre de résolution des identifiants : `CC_API_TOKEN`, puis
//! `CLEVER_TOKEN` + `CLEVER_SECRET`, puis le fichier. Le consumer qui signe
//! est `CLEVER_CONSUMER_KEY`/`CLEVER_CONSUMER_SECRET`, sinon celui de
//! clever-tools (constantes publiques du SDK).

use std::path::PathBuf;

use clevercloud_sdk::Credentials;
use serde::{Deserialize, Serialize};

use crate::exec::platform::Api;

/// Consumer OAuth 1.0. Celui de l'utilisateur (créé dans la console avec
/// la base URL du serveur : danse classique avec callback) ou, à défaut,
/// celui de clever-tools, qui refuse tout callback (« OAuth callback is
/// invalid ») et passe par la page `cli-oauth` de la console.
#[derive(Clone)]
pub struct Consumer {
    pub key: String,
    pub secret: String,
    /// Consumer de clever-tools : connexion par la console.
    pub builtin: bool,
}

impl Consumer {
    pub fn from_env() -> Self {
        match (std::env::var("CLEVER_CONSUMER_KEY"), std::env::var("CLEVER_CONSUMER_SECRET")) {
            (Ok(key), Ok(secret)) if !key.is_empty() && !secret.is_empty() => Consumer { key, secret, builtin: false },
            _ => Consumer { key: clevercloud_sdk::DEFAULT_CONSUMER_KEY.into(), secret: clevercloud_sdk::DEFAULT_CONSUMER_SECRET.into(), builtin: true },
        }
    }
}

/// Jetons gardés sur le poste.
#[derive(Serialize, Deserialize, Clone)]
pub struct Stored {
    pub token: String,
    pub secret: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expiration_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// `~/.config/clever-cloud/clever-compose.json` : le répertoire de
/// clever-tools, un fichier à nous (jamais `clever-tools.json`).
pub fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(".")).join(".config"));
    base.join("clever-cloud").join("clever-compose.json")
}

/// Un échec qui ressemble à des identifiants absents, invalides ou expirés.
pub fn looks_like_auth_failure(msg: &str) -> bool {
    let m = msg.to_ascii_lowercase();
    m.contains("identifiants absents") || m.contains("invalid-token") || m.contains("invalid token") || m.contains("jetons refusés") || m.contains("401 unauthorized") || m.contains("aucun identifiant")
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
    std::fs::write(&path, text).map_err(|e| format!("{} : {e}", path.display()))?;
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
    if let Ok(c) = crate::exec::platform::credentials_from_env() {
        return Ok((c, Source::Env));
    }
    if let Some(s) = load() {
        let consumer = Consumer::from_env();
        return Ok((Credentials::OAuth1 { token: s.token, secret: s.secret, consumer_key: consumer.key, consumer_secret: consumer.secret }, Source::File(config_path())));
    }
    Err("identifiants absents : `clever-compose login`, ou CC_API_TOKEN, ou CLEVER_TOKEN et CLEVER_SECRET dans l'environnement".into())
}

/// `cli_token` aléatoire (20 octets, base64url), comme clever-tools.
pub fn cli_token() -> Result<String, String> {
    let mut bytes = [0u8; 20];
    getrandom::fill(&mut bytes).map_err(|e| format!("aléa : {e}"))?;
    use base64::Engine;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

pub fn console_login_url(cli_token: &str) -> String {
    format!("https://console.clever-cloud.com/cli-oauth?cli_version={}&cli_token={}", env!("CARGO_PKG_VERSION"), cli_token)
}

/// Attend que la console dépose les jetons de `cli_token` (2 s × `tries`).
pub async fn poll_cli_tokens(cli_token: &str, tries: u32, mut on_wait: impl FnMut(u32)) -> Result<Stored, String> {
    let url = format!("{}/v2/self/cli_tokens?cli_token={cli_token}", clevercloud_sdk::PUBLIC_ENDPOINT);
    let client = reqwest::Client::new();
    for i in 0..tries {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let resp = client.get(&url).send().await.map_err(|e| format!("cli_tokens : {e}"))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            on_wait(i);
            continue;
        }
        if !resp.status().is_success() {
            return Err(format!("cli_tokens : {}", resp.status()));
        }
        let v: serde_json::Value = resp.json().await.map_err(|e| format!("cli_tokens : {e}"))?;
        let (Some(token), Some(secret)) = (v["token"].as_str(), v["secret"].as_str()) else { return Err(format!("cli_tokens : réponse inattendue : {v}")) };
        return Ok(Stored { token: token.into(), secret: secret.into(), expiration_date: v["expirationDate"].as_str().map(str::to_string), user_id: None, email: None });
    }
    Err("connexion non terminée dans la console en 2 minutes : recommencez".into())
}

/// Vérifie des jetons auprès de l'API et complète l'utilisateur.
pub async fn verify(stored: &mut Stored, consumer: &Consumer) -> Result<Api, String> {
    let creds = Credentials::OAuth1 { token: stored.token.clone(), secret: stored.secret.clone(), consumer_key: consumer.key.clone(), consumer_secret: consumer.secret.clone() };
    let api = Api::new(String::new(), creds);
    let me = api.whoami().await.map_err(|e| format!("jetons obtenus mais refusés par l'API : {e}"))?;
    stored.user_id = me.0;
    stored.email = me.1;
    Ok(api)
}

/// La danse complète depuis un terminal : URL affichée (et ouverte),
/// attente, vérification, enregistrement.
pub async fn login(open_browser: bool) -> Result<Stored, String> {
    let consumer = Consumer::from_env();
    if !consumer.builtin {
        return Err("`login` passe par la console avec le consumer de clever-tools : retirez CLEVER_CONSUMER_KEY/SECRET, ou fournissez directement CLEVER_TOKEN et CLEVER_SECRET obtenus avec votre consumer".into());
    }
    let token = cli_token()?;
    let url = console_login_url(&token);
    eprintln!("Ouvrez cette page pour autoriser clever-compose :\n  {url}");
    if open_browser {
        let _ = std::process::Command::new("xdg-open").arg(&url).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
    }
    let mut stored = poll_cli_tokens(&token, 60, |i| {
        if i > 0 && i % 10 == 0 {
            eprintln!("  en attente de l'autorisation dans le navigateur…");
        }
    })
    .await?;
    verify(&mut stored, &consumer).await?;
    save(&stored)?;
    Ok(stored)
}
