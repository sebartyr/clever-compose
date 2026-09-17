//! Embarque le shim (statique musl) et l'interface (build Vite) dans la
//! CLI. Le script compile les deux lui-même. Le shim : `cargo build -p
//! cc-compose-shim --profile shim --target x86_64-unknown-linux-musl`. L'interface (feature `ui`) :
//! `npm ci` puis `npm run build` dans `web/`, sortie `web/dist`
//! embarquée par `rust-embed` ; `CLEVER_COMPOSE_UI_DIST` désigne un `dist`
//! déjà bâti et court-circuite npm. Sans npm, un `dist` existant est
//! réutilisé avec un avertissement ; sinon le build échoue clairement.
//!
//! Le shim : le script le compile
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
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("cc-shim");
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    if std::env::var_os("CARGO_FEATURE_UI").is_some() {
        build_ui(&root);
    }
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

/// Bâtit l'interface avec Vite dans `web/dist` (embarqué par
/// `src/web/assets.rs`). `CLEVER_COMPOSE_UI_DIST` : un `dist` déjà bâti,
/// copié à la place.
fn build_ui(root: &std::path::Path) {
    let web = root.join("web");
    let dist = web.join("dist");
    for f in ["src", "index.html", "package.json", "package-lock.json", "vite.config.js"] {
        println!("cargo:rerun-if-changed={}", web.join(f).display());
    }
    println!("cargo:rerun-if-env-changed=CLEVER_COMPOSE_UI_DIST");
    if let Some(given) = std::env::var_os("CLEVER_COMPOSE_UI_DIST").map(PathBuf::from) {
        println!("cargo:rerun-if-changed={}", given.display());
        if given.join("index.html").is_file() {
            let _ = std::fs::remove_dir_all(&dist);
            copy_tree(&given, &dist).expect("copie de CLEVER_COMPOSE_UI_DIST");
            return;
        }
        println!("cargo:warning=CLEVER_COMPOSE_UI_DIST={} sans index.html, build Vite", given.display());
    }
    match npm_build(&web) {
        Ok(()) => {}
        Err(e) if dist.join("index.html").is_file() => println!("cargo:warning=interface non rebâtie : {e} ; web/dist existant embarqué"),
        Err(e) => panic!("interface (feature `ui`) : {e} — installez Node/npm, ou passez un `dist` bâti par CLEVER_COMPOSE_UI_DIST, ou compilez sans interface (--no-default-features --features api)"),
    }
}

fn npm_build(web: &std::path::Path) -> Result<(), String> {
    let run = |args: &[&str]| -> Result<(), String> {
        let output = std::process::Command::new("npm").args(args).current_dir(web).env("CI", "1").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped()).output().map_err(|e| format!("npm {} : {e}", args[0]))?;
        if output.status.success() {
            return Ok(());
        }
        let err = String::from_utf8_lossy(&output.stderr);
        let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
        Err(format!("npm {} : {last}", args.join(" ")))
    };
    if !web.join("node_modules").is_dir() {
        run(&["ci", "--no-audit", "--no-fund"])?;
    }
    run(&["run", "build"])
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_tree(&e.path(), &target)?;
        } else {
            std::fs::copy(e.path(), target)?;
        }
    }
    Ok(())
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
