//! Rendu terminal : couleurs quand la sortie est un terminal (sauf
//! `NO_COLOR`), diagnostics mis en forme, journal des actions avec le verbe
//! en évidence, et indicateur de progression pendant les attentes.

use std::io::{IsTerminal, Write};

/// Préfixe d'une ligne de progression : rendue en place sur un terminal
/// (spinner, temps écoulé), une seule fois sinon.
pub const PROGRESS: &str = "\u{1}";

pub struct Ui {
    color: bool,
    tty: bool,
    /// Ligne de progression affichée, à effacer avant la suivante.
    in_progress: bool,
    tick: usize,
    last_progress_key: String,
}

impl Ui {
    pub fn stderr() -> Ui {
        let tty = std::io::stderr().is_terminal();
        Ui { color: tty && std::env::var_os("NO_COLOR").is_none(), tty, in_progress: false, tick: 0, last_progress_key: String::new() }
    }

    fn paint(&self, code: &str, s: &str) -> String {
        if self.color { format!("\x1b[{code}m{s}\x1b[0m") } else { s.to_string() }
    }

    fn clear_progress(&mut self, out: &mut impl Write) {
        if self.in_progress {
            let _ = write!(out, "\r\x1b[2K");
            self.in_progress = false;
        }
    }

    /// Un diagnostic rendu (`fichier:l:c: sévérité [code] chemin: message`).
    pub fn diagnostic(&mut self, line: &str) {
        let mut out = std::io::stderr().lock();
        self.clear_progress(&mut out);
        let _ = writeln!(out, "{}", self.format_diagnostic(line));
    }

    fn format_diagnostic(&self, line: &str) -> String {
        let (sev, color) = if line.contains(": erreur [") || line.starts_with("erreur [") {
            ("erreur", "1;31")
        } else if line.contains(": avertissement [") || line.starts_with("avertissement [") {
            ("avertissement", "1;33")
        } else {
            return line.to_string();
        };
        let Some(i) = line.find(sev) else { return line.to_string() };
        let (pos, rest) = line.split_at(i);
        let rest = &rest[sev.len()..];
        // `[code] chemin: message`
        let (code, tail) = match rest.trim_start().strip_prefix('[').and_then(|r| r.split_once(']')) {
            Some((c, t)) => (format!("[{c}]"), t),
            None => (String::new(), rest),
        };
        let (path, message) = match tail.split_once(": ") {
            Some((p, m)) => (p.trim(), m),
            None => ("", tail.trim()),
        };
        let mut s = String::new();
        if !pos.is_empty() {
            s.push_str(&self.paint("2", pos.trim_end()));
            s.push(' ');
        }
        s.push_str(&self.paint(color, sev));
        s.push(' ');
        s.push_str(&self.paint("2", &code));
        if !path.is_empty() {
            s.push(' ');
            s.push_str(&self.paint("36", path));
        }
        s.push_str(": ");
        s.push_str(message);
        s
    }

    /// Une ligne du journal d'actions.
    pub fn log(&mut self, line: &str) {
        let mut out = std::io::stderr().lock();
        if let Some(msg) = line.strip_prefix(PROGRESS) {
            // Clé = le message sans le temps écoulé entre parenthèses.
            let key = msg.rsplit_once(" (").map(|(k, _)| k).unwrap_or(msg).to_string();
            if self.tty {
                const SPIN: [char; 4] = ['|', '/', '-', '\\'];
                self.tick = self.tick.wrapping_add(1);
                let _ = write!(out, "\r\x1b[2K{} {}", self.paint("36", &SPIN[self.tick % 4].to_string()), msg);
                let _ = out.flush();
                self.in_progress = true;
            } else if key != self.last_progress_key {
                let _ = writeln!(out, "  {msg}");
            }
            self.last_progress_key = key;
            return;
        }
        self.clear_progress(&mut out);
        self.last_progress_key.clear();
        let _ = writeln!(out, "{}", self.format_log(line));
    }

    fn format_log(&self, line: &str) -> String {
        if line.starts_with("  ") {
            return self.paint("2", line);
        }
        if let Some(rest) = line.strip_prefix("== ") {
            return self.paint("1", &format!("── {rest}"));
        }
        if let Some(rest) = line.strip_prefix("erreur : ") {
            return format!("{} {rest}", self.paint("1;31", "erreur :"));
        }
        // `<kind> <nom> : <verbe...>` : le verbe coloré selon son sens.
        let Some((head, verb)) = line.split_once(" : ") else { return line.to_string() };
        let has = |words: &[&str]| words.iter().any(|w| verb.contains(w));
        let color = if has(&["supprimé", "à supprimer"]) {
            "31"
        } else if has(&["existe", "déjà", "inchangé", "sauté", "conservé", "ignoré"]) {
            "2"
        } else if has(&["attente", "redéploiement", "à créer", "à lier", "à mettre", "exécution locale", "homonyme"]) {
            "33"
        } else if has(&["créé", "déployée", "lié", "poussé", "mis à jour", "ajouté", "terminé", "présente", "prêt"]) {
            "32"
        } else {
            "0"
        };
        let (kind, name) = match head.strip_prefix("network group ") {
            Some(n) => ("network group", n),
            None => head.split_once(' ').unwrap_or((head, "")),
        };
        format!("{} {} : {}", self.paint("1", kind), name, self.paint(color, verb))
    }

    /// Ligne vide de séparation.
    pub fn blank(&mut self) {
        let mut out = std::io::stderr().lock();
        self.clear_progress(&mut out);
        let _ = writeln!(out);
    }

    /// Bilan final.
    pub fn done(&mut self, ok: bool, msg: &str) {
        let mut out = std::io::stderr().lock();
        self.clear_progress(&mut out);
        let _ = writeln!(out, "{} {msg}", if ok { self.paint("1;32", "✔") } else { self.paint("1;31", "✘") });
    }
}
