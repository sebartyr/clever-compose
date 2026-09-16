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

pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() || path == "index.html" { index_name() } else { path };
    match Assets::get(path) {
        Some(f) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref().to_string()), (header::CACHE_CONTROL, "no-cache".to_string())], Body::from(f.data.into_owned())).into_response()
        }
        // Application mono-page : les chemins inconnus servent l'index.
        None if !path.contains('.') => match Assets::get(index_name()) {
            Some(f) => ([(header::CONTENT_TYPE, "text/html; charset=utf-8".to_string())], Body::from(f.data.into_owned())).into_response(),
            None => (StatusCode::NOT_FOUND, "interface absente du binaire").into_response(),
        },
        None => (StatusCode::NOT_FOUND, "").into_response(),
    }
}

fn index_name() -> &'static str {
    if cfg!(feature = "ui-bundled") { "index.html" } else { "index.cdn.html" }
}
