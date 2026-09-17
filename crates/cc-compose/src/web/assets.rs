//! Fichiers de l'interface, embarqués dans le binaire : `web/dist`, la
//! sortie de Vite bâtie par `build.rs` (dépendances incluses, aucun accès
//! réseau à l'usage).

use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

#[derive(rust_embed::Embed)]
#[folder = "../../web/dist/"]
struct Assets;

/// Politique de sécurité de la page : tout vient de la page elle-même ;
/// `unsafe-eval` pour le compilateur de templates de Vue (templates en
/// chaînes, CLAUDE.md §3), `unsafe-inline` pour les styles posés par les
/// composants ; jamais d'inclusion dans un cadre (les jetons sont dans
/// l'onglet), aucun formulaire vers ailleurs.
const CSP: &str = "default-src 'self' data: blob:; script-src 'self' 'unsafe-eval'; style-src 'self' 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'";

/// En-têtes de sécurité des fichiers de l'interface.
fn secured(mut r: Response) -> Response {
    let h = r.headers_mut();
    h.insert(header::CONTENT_SECURITY_POLICY, CSP.parse().unwrap());
    h.insert(header::X_CONTENT_TYPE_OPTIONS, "nosniff".parse().unwrap());
    h.insert(header::X_FRAME_OPTIONS, "DENY".parse().unwrap());
    h.insert(header::REFERRER_POLICY, "no-referrer".parse().unwrap());
    r
}

pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match Assets::get(path) {
        Some(f) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            secured(([(header::CONTENT_TYPE, mime.as_ref().to_string()), (header::CACHE_CONTROL, "no-cache".to_string())], Body::from(f.data.into_owned())).into_response())
        }
        // Application mono-page : les chemins inconnus servent l'index.
        None if !path.contains('.') => match Assets::get("index.html") {
            Some(f) => secured(([(header::CONTENT_TYPE, "text/html; charset=utf-8".to_string())], Body::from(f.data.into_owned())).into_response()),
            None => (StatusCode::NOT_FOUND, "interface absente du binaire").into_response(),
        },
        None => (StatusCode::NOT_FOUND, "").into_response(),
    }
}
