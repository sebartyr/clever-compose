//! Dépôt synthétique d'une application : ce que les trois backends poussent
//! ou décrivent. Contenu déterministe (ordre fixe, pas de date) pour que
//! deux `up` sans changement donnent le même arbre git (CLAUDE.md §7).

use std::path::Path as FsPath;

use cc_compose_model::plan::{App, Source};
use cc_compose_model::shim::{ShimConfig, CONFIG_PATH, IDS_PATH};

/// Un fichier du dépôt synthétique.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    /// Chemin dans le dépôt.
    pub path: String,
    pub content: Vec<u8>,
    pub executable: bool,
}

const SHIM_BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/cc-shim"));

/// Le binaire du shim : embarqué à la compilation, ou lu sur le disque via
/// `CC_SHIM_PATH` (développement, compilation croisée pénible en local).
pub fn shim_binary() -> Result<Vec<u8>, String> {
    if let Some(p) = std::env::var_os("CC_SHIM_PATH") {
        return std::fs::read(&p).map_err(|e| format!("CC_SHIM_PATH : {} : {e}", FsPath::new(&p).display()));
    }
    if SHIM_BYTES.is_empty() {
        return Err("shim non embarqué dans cette compilation : compilez-le (`cargo build -p cc-compose-shim --profile shim --target x86_64-unknown-linux-musl`) ou indiquez CC_SHIM_PATH".into());
    }
    Ok(SHIM_BYTES.to_vec())
}

/// Nom du Dockerfile généré quand le service a un `build:` : le Dockerfile
/// de l'utilisateur, prolongé par notre bloc. `CC_DOCKERFILE` le désigne.
pub const CONTEXT_DOCKERFILE: &str = "Dockerfile.cc";

/// Dockerfile généré : `FROM <image>` pour un service `image:`, ou le
/// Dockerfile de l'utilisateur tel quel pour un `build:` ; puis les fichiers
/// copiés, la résolution de l'utilisateur en uid/gid, le shim et sa
/// configuration, l'entrypoint.
pub fn dockerfile(app: &App, project_dir: &FsPath) -> Result<String, String> {
    let mut out = String::new();
    let generated: &[cc_compose_model::plan::GeneratedFile] = match &app.source {
        Source::Synthetic { generated, .. } => generated,
        _ => &[],
    };
    let files = match &app.source {
        Source::Synthetic { image, files, .. } => {
            out.push_str("# Généré par clever-compose ; ne pas éditer.\n");
            out.push_str(&format!("FROM {image}\n"));
            files
        }
        Source::Context { context, dockerfile, files } => {
            let path = project_dir.join(context.trim_start_matches("./")).join(dockerfile);
            let user = std::fs::read_to_string(&path).map_err(|e| format!("{} : {e}", path.display()))?;
            out.push_str(&user);
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("\n# Bloc ajouté par clever-compose ; ne pas éditer.\n");
            files
        }
    };
    for (i, f) in files.iter().enumerate() {
        out.push_str(&format!("COPY cc-files/{i} {}\n", f.target));
    }
    for (i, g) in generated.iter().enumerate() {
        out.push_str(&format!("COPY cc-generated/{i} {}\n", g.target));
    }
    out.push_str("USER root\n");
    if let Some(user) = &app.shim.user {
        // Résolution du nom en uid/gid au build, avec la libc de l'image
        // (getpwnam n'est pas fiable en musl statique, CLAUDE.md §8).
        out.push_str(&format!(
            "RUN printf '%s:%s:%s\\n' \"$(id -u {user})\" \"$(id -g {user})\" \"$(id -G {user} | tr ' ' ',')\" > {IDS_PATH}\n"
        ));
    }
    out.push_str("COPY cc-shim /cc-shim\n");
    out.push_str(&format!("COPY cc-shim.json {CONFIG_PATH}\n"));
    out.push_str("ENTRYPOINT [\"/cc-shim\"]\n");
    Ok(out)
}

/// Configuration figée du shim, sérialisée de façon stable.
pub fn shim_config_json(shim: &ShimConfig) -> String {
    let mut s = serde_json::to_string_pretty(shim).unwrap_or_default();
    s.push('\n');
    s
}

/// Les fichiers ajoutés par clever-compose, dans un ordre fixe. Pour un
/// `build:`, le contexte de l'utilisateur s'y ajoute tel quel (exécuteur).
pub fn files(app: &App, project_dir: &FsPath) -> Result<Vec<File>, String> {
    let dockerfile_name = match &app.source {
        Source::Synthetic { .. } => "Dockerfile",
        Source::Context { .. } => CONTEXT_DOCKERFILE,
    };
    let mut out = vec![
        File { path: dockerfile_name.into(), content: dockerfile(app, project_dir)?.into_bytes(), executable: false },
        File { path: "cc-shim.json".into(), content: shim_config_json(&app.shim).into_bytes(), executable: false },
        File { path: "cc-shim".into(), content: shim_binary()?, executable: true },
    ];
    let copied = match &app.source {
        Source::Synthetic { files, .. } | Source::Context { files, .. } => files,
    };
    for (i, f) in copied.iter().enumerate() {
        let abs = project_dir.join(f.source.trim_start_matches("./"));
        let content = std::fs::read(&abs).map_err(|e| format!("{} : {e}", abs.display()))?;
        out.push(File { path: format!("cc-files/{i}"), content, executable: false });
    }
    if let Source::Synthetic { generated, .. } = &app.source {
        for (i, g) in generated.iter().enumerate() {
            out.push(File { path: format!("cc-generated/{i}"), content: g.content.clone().into_bytes(), executable: g.executable });
        }
    }
    Ok(out)
}
