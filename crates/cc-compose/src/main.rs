//! CLI clever-compose. Squelette : le parsing et le plan arrivent en phase 1,
//! l'embarquement du shim via `include_bytes!` en phase 3.

fn main() {
    eprintln!("clever-compose : phase 0, rien à exécuter encore");
    std::process::exit(2);
}
