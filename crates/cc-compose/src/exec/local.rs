//! Exécution d'une tâche d'initialisation ou de reprise depuis le poste
//! (`run: local`, `x-clever.init: local`, `seed --local`) : même script,
//! mêmes fichiers et mêmes variables que la tâche plateforme, mais lancés
//! ici, contre l'addon. Avec Docker si présent (l'image du plan, donc le
//! client exact), sinon directement avec le client du poste.

use std::path::Path as FsPath;
use std::process::{Command, Stdio};

use cc_compose_model::plan::*;

use super::platform::Platform;
use super::reconcile::{Log, State};
use crate::plan::initdb;

pub async fn run_task<P: Platform>(app: &App, state: &State, platform: &P, project_dir: &FsPath, log: &mut Log) -> Result<(), String> {
    // L'addon dont la tâche dépend fournit ses variables, comme le lien.
    let addon_service = app.depends_on.iter().find_map(|d| d.0.strip_prefix("addon/")).ok_or_else(|| format!("tâche {} : aucun addon dans depends_on", app.name))?;
    let addon_id = state.addon_ids.get(addon_service).ok_or_else(|| format!("tâche {} : addon `{addon_service}` inconnu", app.name))?;
    let mut env: Vec<(String, String)> = platform.addon_env(addon_id).await?.into_iter().map(|e| (e.name, e.value)).collect();
    for e in &app.env {
        let value = e.value.0.iter().map(|seg| match seg { Segment::Literal(l) => l.clone(), Segment::Ref(_) => String::new() }).collect::<String>();
        env.push((e.key.clone(), value));
    }

    // Les fichiers de la tâche, tels que le Dockerfile les copierait :
    // scripts sous /cc-init, runner /cc-init.sh.
    let Source::Synthetic { image, files, generated, .. } = &app.source else { return Err(format!("tâche {} : source inattendue", app.name)) };
    let work = std::env::temp_dir().join(format!("clever-compose-local-{}", app.name));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(work.join("cc-init")).map_err(|e| e.to_string())?;
    for f in files {
        let Some(name) = f.target.strip_prefix("/cc-init/") else { continue };
        let abs = project_dir.join(f.source.trim_start_matches("./"));
        std::fs::copy(&abs, work.join("cc-init").join(name)).map_err(|e| format!("{} : {e}", abs.display()))?;
    }
    let runner = work.join("cc-init.sh");
    let script = generated.iter().find(|g| g.target == "/cc-init.sh").ok_or_else(|| format!("tâche {} : pas de script d'exécution", app.name))?;
    std::fs::write(&runner, &script.content).map_err(|e| e.to_string())?;

    let mut cmd = if docker_usable() {
        log(&format!("tâche {} : exécution locale avec Docker ({image})", app.name));
        let mut c = Command::new("docker");
        c.args(["run", "--rm", "-i"]);
        for (k, v) in &env {
            c.arg("-e").arg(format!("{k}={v}"));
        }
        c.arg("-v").arg(format!("{}:/cc-init:ro", work.join("cc-init").display()));
        c.arg("-v").arg(format!("{}:/cc-init.sh:ro", runner.display()));
        c.arg("--entrypoint").arg("sh").arg(image).arg("/cc-init.sh");
        c
    } else {
        let client = initdb::client(provider_of(image));
        if !which(client) {
            return Err(format!("tâche {} : ni `docker` ni `{client}` sur ce poste ; installez l'un des deux, ou retirez `local` pour laisser la plateforme exécuter la tâche", app.name));
        }
        log(&format!("tâche {} : exécution locale avec `{client}`", app.name));
        let mut c = Command::new("sh");
        c.arg(&runner);
        c.env("CC_INIT_DIR", work.join("cc-init"));
        c.env("TMPDIR", &work);
        for (k, v) in &env {
            c.env(k, v);
        }
        c
    };
    let status = cmd.stdin(Stdio::null()).stdout(Stdio::inherit()).stderr(Stdio::inherit()).status().map_err(|e| format!("tâche {} : {e}", app.name))?;
    let _ = std::fs::remove_dir_all(&work);
    if !status.success() {
        return Err(format!("tâche {} : échec local ({status})", app.name));
    }
    log(&format!("tâche {} : terminée localement", app.name));
    Ok(())
}

/// Le fournisseur, retrouvé depuis l'image du runner.
fn provider_of(image: &str) -> &'static str {
    let name = image.split(':').next().unwrap_or("");
    match name {
        "postgres" => "postgresql-addon",
        "mysql" => "mysql-addon",
        "mongo" => "mongodb-addon",
        "redis" => "redis-addon",
        "curlimages/curl" => "es-addon",
        _ => "",
    }
}

/// Docker présent et démon joignable (un binaire sans accès au socket ne
/// compte pas).
fn docker_usable() -> bool {
    which("docker") && Command::new("docker").args(["version", "--format", "{{.Server.Version}}"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH").map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file())).unwrap_or(false)
}
