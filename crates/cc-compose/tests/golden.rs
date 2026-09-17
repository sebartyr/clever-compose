//! Golden files du plan (CLAUDE.md §11) : chaque cas du corpus doté d'une
//! fixture d'images (`images.json`) produit `plan.json` et
//! `plan.diagnostics.txt`, figés dans le dépôt. Régénération :
//! `CLEVER_COMPOSE_REGEN_GOLDEN=1 cargo test`.
//!
//! `plan-options.json` (optionnel) : `{"allow_ephemeral": true,
//! "private_registries": ["ghcr.io"]}` (les registres « avec identifiants »
//! sont fixés ici pour que le golden ne dépende pas du poste).

use std::path::{Path, PathBuf};

use cc_compose::compose::project::Options;
use cc_compose::diagnostic::Diagnostics;
use cc_compose::plan::cli::{fixture_from_file, run, PlanOptions};
use pretty_assertions::assert_eq;

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")
}

#[derive(Default, serde::Deserialize)]
struct PlanTestOptions {
    #[serde(default)]
    allow_ephemeral: bool,
    #[serde(default)]
    private_registries: Vec<String>,
}

#[test]
fn corpus_plans_match_golden_files() {
    let regen = std::env::var_os("CLEVER_COMPOSE_REGEN_GOLDEN").is_some();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(corpus_dir()).unwrap().filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.join("images.json").is_file()).collect();
    entries.sort();
    assert!(!entries.is_empty());
    let mut failures = Vec::new();
    for dir in entries {
        let fixture = fixture_from_file(&dir.join("images.json")).unwrap();
        let topts: PlanTestOptions = std::fs::read_to_string(dir.join("plan-options.json")).ok().map(|s| serde_json::from_str(&s).unwrap()).unwrap_or_default();
        let opts = PlanOptions {
            project: Options { project_dir: dir.clone(), files: Vec::new(), project_name: None, env_files: Vec::new(), profiles: Vec::new(), confine: false },
            allow_ephemeral: topts.allow_ephemeral,
            private_registries: Some(topts.private_registries.clone()),
        };
        let mut diags = Diagnostics::default();
        let plan = run(&opts, &fixture, &mut diags);
        let plan_json = plan.map(|p| serde_json::to_string_pretty(&p).unwrap() + "\n").unwrap_or_else(|| "null\n".into());
        let diag_text = diags.render_all().join("\n") + "\n";
        let plan_path = dir.join("plan.json");
        let diag_path = dir.join("plan.diagnostics.txt");
        if regen || !plan_path.exists() {
            std::fs::write(&plan_path, &plan_json).unwrap();
            std::fs::write(&diag_path, &diag_text).unwrap();
            continue;
        }
        let expected_plan = std::fs::read_to_string(&plan_path).unwrap();
        let expected_diag = std::fs::read_to_string(&diag_path).unwrap_or_default();
        let r = std::panic::catch_unwind(|| {
            assert_eq!(plan_json, expected_plan, "plan de {}", dir.display());
            assert_eq!(diag_text, expected_diag, "diagnostics de {}", dir.display());
        });
        if r.is_err() {
            failures.push(dir.display().to_string());
        }
    }
    assert!(failures.is_empty(), "golden files divergents : {failures:?}");
}
