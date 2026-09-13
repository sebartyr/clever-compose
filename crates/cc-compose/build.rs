//! Embarque le shim compilé (statique musl) dans la CLI. Chemin cherché :
//! `$CC_SHIM_BIN`, sinon `target/x86_64-unknown-linux-musl/shim/cc-shim` du
//! workspace. Absent, un fichier vide est embarqué et l'outil le signale à
//! l'usage (`CC_SHIM_PATH` permet alors de charger un shim depuis le disque).

use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("cc-shim");
    println!("cargo:rerun-if-env-changed=CC_SHIM_BIN");
    let candidates: Vec<PathBuf> = std::env::var_os("CC_SHIM_BIN")
        .map(PathBuf::from)
        .into_iter()
        .chain(std::iter::once(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/x86_64-unknown-linux-musl/shim/cc-shim")))
        .collect();
    for c in &candidates {
        println!("cargo:rerun-if-changed={}", c.display());
        if c.is_file() {
            std::fs::copy(c, &out).expect("copie du shim");
            println!("cargo:rustc-env=CC_SHIM_EMBEDDED=1");
            return;
        }
    }
    std::fs::write(&out, []).expect("shim vide");
    println!("cargo:warning=cc-shim non trouvé : compilez-le avec `cargo build -p cc-compose-shim --profile shim --target x86_64-unknown-linux-musl` (CLI utilisable avec CC_SHIM_PATH)");
}
