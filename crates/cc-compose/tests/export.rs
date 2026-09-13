//! Golden files des émetteurs (CLAUDE.md §11 et phase 2) : pour chaque cas
//! du corpus dont le plan existe, `export/main.tf.json` et
//! `export/bootstrap.sh` sont figés (`CC_REGEN_GOLDEN=1`). Puis
//! `terraform validate` sur chaque `main.tf.json` quand `terraform` est
//! disponible (PATH ou `~/.local/bin`), avec le cache de plugins partagé.

use std::path::{Path, PathBuf};
use std::process::Command;

use cc_compose::compose::project::Options;
use cc_compose::diagnostic::Diagnostics;
use cc_compose::emit::{script, terraform};
use cc_compose::plan::cli::{fixture_from_file, run, PlanOptions};
use pretty_assertions::assert_eq;

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus")
}

#[derive(Default, serde::Deserialize)]
struct PlanTestOptions {
    #[serde(default)]
    allow_ephemeral: bool,
}

fn cases() -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(corpus_dir()).unwrap().filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.join("images.json").is_file()).collect();
    entries.sort();
    entries
}

#[test]
fn corpus_exports_match_golden_files() {
    let regen = std::env::var_os("CC_REGEN_GOLDEN").is_some();
    let mut failures = Vec::new();
    for dir in cases() {
        let fixture = fixture_from_file(&dir.join("images.json")).unwrap();
        let topts: PlanTestOptions = std::fs::read_to_string(dir.join("plan-options.json")).ok().map(|s| serde_json::from_str(&s).unwrap()).unwrap_or_default();
        let opts = PlanOptions {
            project: Options { project_dir: dir.clone(), files: Vec::new(), project_name: None, env_files: Vec::new(), profiles: Vec::new() },
            allow_ephemeral: topts.allow_ephemeral,
        };
        let mut diags = Diagnostics::default();
        let Some(plan) = run(&opts, &fixture, &mut diags) else { continue };
        let out_dir = dir.join("export");
        let tf = terraform::emit(&plan, &terraform::Options { out_dir: &out_dir, project_dir: &dir }, &mut diags);
        let tf_text = serde_json::to_string_pretty(&tf).unwrap() + "\n";
        // Le HCL vit dans `export/hcl/` : ses `file()` sont relatifs à ce
        // répertoire.
        let hcl_dir = out_dir.join("hcl");
        let tf_for_hcl = terraform::emit(&plan, &terraform::Options { out_dir: &hcl_dir, project_dir: &dir }, &mut Diagnostics::default());
        let hcl_text = cc_compose::emit::hcl::render(&tf_for_hcl, &terraform::schema());
        let sh_text = script::emit(&plan, &script::Options { out_dir: &out_dir, project_dir: &dir }, &mut diags);
        std::fs::create_dir_all(&out_dir).unwrap();
        let tf_path = out_dir.join("main.tf.json");
        let hcl_path = out_dir.join("hcl/main.tf");
        let sh_path = out_dir.join("bootstrap.sh");
        if regen || !tf_path.exists() {
            std::fs::write(&tf_path, &tf_text).unwrap();
            std::fs::create_dir_all(out_dir.join("hcl")).unwrap();
            std::fs::write(&hcl_path, &hcl_text).unwrap();
            std::fs::write(&sh_path, &sh_text).unwrap();
            continue;
        }
        let r = std::panic::catch_unwind(|| {
            assert_eq!(tf_text, std::fs::read_to_string(&tf_path).unwrap(), "terraform de {}", dir.display());
            assert_eq!(sh_text, std::fs::read_to_string(&sh_path).unwrap(), "script de {}", dir.display());
            assert_eq!(hcl_text, std::fs::read_to_string(&hcl_path).unwrap_or_default(), "hcl de {}", dir.display());
        });
        if r.is_err() {
            failures.push(dir.display().to_string());
        }
    }
    assert!(failures.is_empty(), "exports divergents : {failures:?}");
}

fn terraform_bin() -> Option<PathBuf> {
    if let Ok(out) = Command::new("terraform").arg("version").output() {
        if out.status.success() {
            return Some(PathBuf::from("terraform"));
        }
    }
    let local = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/bin/terraform"))?;
    local.is_file().then_some(local)
}

#[test]
fn corpus_terraform_validates() {
    let Some(tf) = terraform_bin() else {
        eprintln!("terraform absent : validation sautée");
        return;
    };
    let cache = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache/terraform-plugins")).unwrap();
    std::fs::create_dir_all(&cache).unwrap();
    let mut failures = Vec::new();
    for dir in cases() {
        let src = dir.join("export/main.tf.json");
        if !src.is_file() {
            continue;
        }
        // En place : les `file()` des secrets pointent dans le projet.
        // `.terraform*` est ignoré par git. Le JSON est validé, puis le HCL
        // rendu depuis la même structure (`hcl/main.tf`, `file()` relatif).
        let work = dir.join("export");
        let init = Command::new(&tf).args(["init", "-input=false", "-backend=false", "-no-color"]).current_dir(&work).env("TF_PLUGIN_CACHE_DIR", &cache).output().unwrap();
        if !init.status.success() {
            eprintln!("terraform init en échec pour {} :\n{}", dir.display(), String::from_utf8_lossy(&init.stderr));
            failures.push(dir.display().to_string());
            continue;
        }
        let out = Command::new(&tf).args(["validate", "-no-color"]).current_dir(&work).output().unwrap();
        if !out.status.success() {
            eprintln!("terraform validate en échec pour {} :\n{}{}", dir.display(), String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            failures.push(dir.display().to_string());
        }
        // Le HCL rendu, validé à son tour.
        let hcl = work.join("hcl");
        if hcl.join("main.tf").is_file() {
            let init = Command::new(&tf).args(["init", "-input=false", "-backend=false", "-no-color"]).current_dir(&hcl).env("TF_PLUGIN_CACHE_DIR", &cache).output().unwrap();
            let out = Command::new(&tf).args(["validate", "-no-color"]).current_dir(&hcl).output().unwrap();
            if !init.status.success() || !out.status.success() {
                eprintln!("terraform validate (HCL) en échec pour {} :\n{}{}", dir.display(), String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
                failures.push(format!("{} (hcl)", dir.display()));
            }
        }
    }
    assert!(failures.is_empty(), "terraform validate en échec : {failures:?}");
}
