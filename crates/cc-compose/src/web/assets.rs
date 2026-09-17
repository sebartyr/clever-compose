//! Fichiers de l'interface embarqués dans le binaire. Deux modes, choisis au
//! build : `ui-bundled` embarque `web/dist` (à la racine du dépôt, sortie de
//! Vite, dépendances incluses) ; sinon les sources ESM de `web/` sont servies
//! telles quelles
//! et la page charge Vue et @clevercloud/components depuis esm.sh.

use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

#[cfg(feature = "ui-bundled")]
#[derive(rust_embed::Embed)]
#[folder = "../../web/dist/"]
struct Assets;

#[cfg(not(feature = "ui-bundled"))]
#[derive(rust_embed::Embed)]
#[folder = "../../web/"]
#[include = "index.cdn.html"]
#[include = "src/*"]
#[include = "src/**/*"]
struct Assets;

/// Politique de sécurité de la page : scripts et styles de la page
/// elle-même et, hors `ui-bundled`, des deux CDN de `index.cdn.html` ;
/// `unsafe-eval` pour le compilateur de templates de Vue (templates en
/// chaînes, CLAUDE.md §3), `unsafe-inline` pour les styles posés par les
/// composants ; jamais d'inclusion dans un cadre (les jetons sont dans
/// l'onglet), aucun formulaire vers ailleurs.
fn csp() -> &'static str {
    static CSP: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    CSP.get_or_init(|| {
        if cfg!(feature = "ui-bundled") {
            "default-src 'self' data: blob:; script-src 'self' 'unsafe-eval'; style-src 'self' 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'".to_string()
        } else {
            // L'import map de `index.cdn.html` est un script en ligne : admis
            // par son empreinte, calculée sur le fichier embarqué.
            let hash = importmap_hash().map(|h| format!(" '{h}'")).unwrap_or_default();
            format!("default-src 'self' data: blob: https://esm.sh https://cdn.jsdelivr.net; script-src 'self' 'unsafe-eval'{hash} https://esm.sh https://cdn.jsdelivr.net; style-src 'self' 'unsafe-inline' https://esm.sh https://cdn.jsdelivr.net; connect-src 'self' https://esm.sh https://cdn.jsdelivr.net; frame-ancestors 'none'; base-uri 'self'; form-action 'self'")
        }
    })
}

/// `sha256-…` (CSP) du contenu du `<script type="importmap">` de l'index
/// embarqué, tel quel entre les balises.
fn importmap_hash() -> Option<String> {
    use base64::Engine;
    use sha2::Digest;
    let index = Assets::get(index_name())?;
    let text = std::str::from_utf8(&index.data).ok()?;
    let start = text.find("<script type=\"importmap\">")? + "<script type=\"importmap\">".len();
    let end = start + text[start..].find("</script>")?;
    let digest = sha2::Sha256::digest(&text.as_bytes()[start..end]);
    Some(format!("sha256-{}", base64::engine::general_purpose::STANDARD.encode(digest)))
}

/// En-têtes de sécurité des fichiers de l'interface.
fn secured(mut r: Response) -> Response {
    let h = r.headers_mut();
    h.insert(header::CONTENT_SECURITY_POLICY, csp().parse().unwrap());
    h.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    h.insert(header::X_FRAME_OPTIONS, "DENY".parse().unwrap());
    h.insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    r
}

pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() || path == "index.html" { index_name() } else { path };
    match Assets::get(path) {
        Some(f) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            secured(([(header::CONTENT_TYPE, mime.as_ref().to_string()), (header::CACHE_CONTROL, "no-cache".to_string())], Body::from(f.data.into_owned())).into_response())
        }
        // Application mono-page : les chemins inconnus servent l'index.
        None if !path.contains('.') => match Assets::get(index_name()) {
            Some(f) => secured(([(header::CONTENT_TYPE, "text/html; charset=utf-8".to_string())], Body::from(f.data.into_owned())).into_response()),
            None => (StatusCode::NOT_FOUND, "interface absente du binaire").into_response(),
        },
        None => (StatusCode::NOT_FOUND, "").into_response(),
    }
}

fn index_name() -> &'static str {
    if cfg!(feature = "ui-bundled") { "index.html" } else { "index.cdn.html" }
}
