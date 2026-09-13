//! Test différentiel du parsing (CLAUDE.md §11) : `docker compose config`
//! est l'oracle. Chaque répertoire de `tests/corpus/` contient un projet ;
//! `expected.yaml` est la sortie de l'oracle, figée dans le dépôt et
//! régénérée avec `CC_REGEN_ORACLE=1 cargo test` (docker requis).
//!
//! Les deux sorties sont comparées en JSON après canonicalisation.

use std::path::{Path, PathBuf};
use std::process::Command;

use cc_compose::compose::project::{load, Options};
use cc_compose::compose::yaml;
use cc_compose::diagnostic::Diagnostics;
use pretty_assertions::assert_eq;
use serde_json::Value as J;

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")
}

/// Sortie de l'oracle ; `Err` avec son message quand il refuse le projet
/// (figé dans `expected.error`).
fn regen_oracle(dir: &Path) -> Result<String, String> {
    let out = Command::new("docker")
        .args(["compose", "--project-directory"])
        .arg(dir)
        .arg("config")
        .env_remove("COMPOSE_PROJECT_NAME")
        .output()
        .expect("docker compose introuvable");
    if out.status.success() {
        Ok(String::from_utf8(out.stdout).unwrap())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).to_string())
    }
}

/// Canonicalisation avant comparaison. Chaque règle ici est une divergence
/// de forme entre l'oracle et notre arbre, assumée et documentée ; toute
/// divergence de fond doit rester visible.
fn canonical(v: J) -> J {
    match v {
        J::Object(m) => J::Object(m.into_iter().map(|(k, v)| (k, canonical(v))).collect()),
        J::Array(a) => J::Array(a.into_iter().map(canonical).collect()),
        other => other,
    }
}

/// Côté oracle seulement : `docker compose config` double les `$` de sa
/// sortie (`cmd/compose/config.go`) pour qu'elle reste réinterpolable.
fn unescape_dollars(v: J) -> J {
    match v {
        J::String(s) => J::String(s.replace("$$", "$")),
        J::Object(m) => J::Object(m.into_iter().map(|(k, v)| (k, unescape_dollars(v))).collect()),
        J::Array(a) => J::Array(a.into_iter().map(unescape_dollars).collect()),
        other => other,
    }
}

fn ours(dir: &Path) -> Result<J, String> {
    let opts = Options { project_dir: dir.to_path_buf(), files: Vec::new(), project_name: None, env_files: Vec::new(), profiles: Vec::new() };
    let mut diags = Diagnostics::default();
    let project = load(&opts, &mut diags);
    let rendered = diags.render_all().join("\n");
    match project {
        Some(p) if !diags.has_errors() => Ok(canonical(p.tree.to_json())),
        _ => Err(rendered),
    }
}

#[test]
fn corpus_matches_docker_compose_config() {
    let regen = std::env::var_os("CC_REGEN_ORACLE").is_some();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(corpus_dir())
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    entries.sort();
    assert!(!entries.is_empty());
    let mut failures = Vec::new();
    for dir in entries {
        let expected_path = dir.join("expected.yaml");
        let error_path = dir.join("expected.error");
        if regen || (!expected_path.exists() && !error_path.exists()) {
            match regen_oracle(&dir) {
                Ok(out) => {
                    std::fs::write(&expected_path, out).unwrap();
                    let _ = std::fs::remove_file(&error_path);
                }
                Err(msg) => {
                    std::fs::write(&error_path, msg).unwrap();
                    let _ = std::fs::remove_file(&expected_path);
                }
            }
        }
        let actual = ours(&dir);
        if error_path.exists() {
            // L'oracle refuse : nous devons refuser aussi.
            if let Ok(_) = actual {
                eprintln!("=== {} : l'oracle refuse ({}), pas nous", dir.display(), std::fs::read_to_string(&error_path).unwrap().trim());
                failures.push(dir.display().to_string());
            }
            continue;
        }
        let expected = yaml::load(&std::fs::read_to_string(&expected_path).unwrap(), 0).unwrap();
        let expected = canonical(unescape_dollars(expected.to_json()));
        match actual {
            Ok(actual) if actual == expected => {}
            Ok(actual) => {
                eprintln!("=== {} diverge", dir.display());
                let r = std::panic::catch_unwind(|| assert_eq!(actual, expected));
                if r.is_err() {
                    failures.push(dir.display().to_string());
                }
            }
            Err(msg) => failures.push(format!("{} (chargement en échec : {msg})", dir.display())),
        }
    }
    assert!(failures.is_empty(), "cas divergents : {failures:?}");
}
