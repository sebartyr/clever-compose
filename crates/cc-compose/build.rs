//! Embarque le shim (statique musl) dans la CLI. Le script le compile
//! lui-même (`cargo build -p cc-compose-shim --profile shim --target
//! x86_64-unknown-linux-musl`, dans un répertoire `target/shim-build`
//! séparé pour ne pas croiser le verrou du build en cours) et le relance
//! quand ses sources changent. `$CLEVER_COMPOSE_SHIM_BIN` désigne un shim déjà compilé
//! et court-circuite la compilation. Si la cible musl manque, un fichier
//! vide est embarqué et l'outil le signale à l'usage (`CLEVER_COMPOSE_SHIM_PATH` permet
//! alors de charger un shim depuis le disque).

use std::path::PathBuf;

fn main() {
    // Empreinte du build (commit, date) pour `--version` et l'interface :
    // les sources du front sont embarquées au build, un binaire ancien se
    // repère ainsi.
    let commit = std::process::Command::new("git").args(["rev-parse", "--short", "HEAD"]).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_else(|| "inconnu".into());
    let dirty = std::process::Command::new("git").args(["status", "--porcelain", "--untracked-files=no"]).output().ok().is_some_and(|o| !o.stdout.is_empty());
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    println!("cargo:rustc-env=CLEVER_COMPOSE_BUILD_STAMP={}{} {}", commit, if dirty { "+" } else { "" }, now);
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");
    println!("cargo:rerun-if-changed=../../web/src");
    println!("cargo:rerun-if-changed=../../web/index.cdn.html");
    println!("cargo:rerun-if-changed=../../web/dist");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("cc-shim");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    println!("cargo:rerun-if-env-changed=CLEVER_COMPOSE_SHIM_BIN");
    println!("cargo:rerun-if-changed={}", root.join("crates/cc-compose-shim/src").display());
    println!("cargo:rerun-if-changed={}", root.join("crates/cc-compose-shim/Cargo.toml").display());
    println!("cargo:rerun-if-changed={}", root.join("crates/cc-compose-model/src").display());
    println!("cargo:rerun-if-changed={}", root.join("Cargo.lock").display());
    if let Some(given) = std::env::var_os("CLEVER_COMPOSE_SHIM_BIN").map(PathBuf::from) {
        println!("cargo:rerun-if-changed={}", given.display());
        if given.is_file() {
            std::fs::copy(&given, &out).expect("copie du shim");
            println!("cargo:rustc-env=CLEVER_COMPOSE_SHIM_EMBEDDED=1");
            return;
        }
        println!("cargo:warning=CLEVER_COMPOSE_SHIM_BIN={} introuvable, compilation du shim", given.display());
    }
    match build_shim(&root) {
        Ok(bin) => {
            std::fs::copy(&bin, &out).expect("copie du shim");
            println!("cargo:rustc-env=CLEVER_COMPOSE_SHIM_EMBEDDED=1");
        }
        Err(e) => {
            std::fs::write(&out, []).expect("shim vide");
            println!("cargo:warning=shim non embarqué : {e} (CLI utilisable avec CLEVER_COMPOSE_SHIM_PATH)");
        }
    }
}

/// Compile le shim pour musl dans `target/shim-build` (à côté du `target`
/// du workspace, ou de `CARGO_TARGET_DIR`) et rend le chemin du binaire.
fn build_shim(root: &std::path::Path) -> Result<PathBuf, String> {
    const TRIPLE: &str = "x86_64-unknown-linux-musl";
    let target_dir = std::env::var_os("CARGO_TARGET_DIR").map(PathBuf::from).unwrap_or_else(|| root.join("target")).join("shim-build");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = std::process::Command::new(cargo);
    cmd.current_dir(root)
        .args(["build", "-p", "cc-compose-shim", "--profile", "shim", "--target", TRIPLE, "--target-dir"])
        .arg(&target_dir)
        // L'environnement du build en cours vise l'hôte : ne pas le propager.
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_BUILD_TARGET")
        .env_remove("CARGO_BUILD_RUSTFLAGS")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CLEVER_COMPOSE_SHIM_BIN")
        .env("CARGO_TERM_COLOR", "never")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    let output = cmd.output().map_err(|e| format!("lancement de cargo : {e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let hint = if err.contains("target may not be installed") || err.contains("can't find crate for `std`") { format!(" — installez la cible : rustup target add {TRIPLE}") } else { String::new() };
        let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
        return Err(format!("cargo build du shim : {last}{hint}"));
    }
    let bin = target_dir.join(TRIPLE).join("shim").join("cc-shim");
    if !bin.is_file() {
        return Err(format!("{} absent après la compilation", bin.display()));
    }
    Ok(bin)
}
