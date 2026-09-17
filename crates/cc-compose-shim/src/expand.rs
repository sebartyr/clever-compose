//! Expansion des variables au démarrage (CLAUDE.md §6.2) : la plateforme
//! n'expanse pas `${X}` entre variables (phase 0, question 7). Les variables
//! listées dans `CLEVER_COMPOSE_EXPAND` voient leurs `${NOM}` remplacés par la
//! valeur de `NOM` dans l'environnement (typiquement les variables injectées
//! par la liaison d'un addon). Une variable absente donne une chaîne vide et
//! un message.

pub const LIST_VAR: &str = "CLEVER_COMPOSE_EXPAND";

pub fn expand_all() -> Vec<(String, String)> {
    let Some(list) = std::env::var_os(LIST_VAR) else { return Vec::new() };
    let mut out = Vec::new();
    for name in list.to_string_lossy().split(',').map(str::trim).filter(|n| !n.is_empty()) {
        let Some(value) = std::env::var_os(name) else { continue };
        let expanded = expand(&value.to_string_lossy());
        out.push((name.to_string(), expanded));
    }
    out
}

pub fn expand(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        match after.find('}') {
            Some(j) => {
                let name = &after[..j];
                match std::env::var(name) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => eprintln!("cc-shim: expansion : variable `{name}` absente, remplacée par une chaîne vide"),
                }
                rest = &after[j + 1..];
            }
            None => {
                out.push_str("${");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}
