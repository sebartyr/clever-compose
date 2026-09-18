//! Rendu terminal : couleurs quand la sortie est un terminal (sauf
//! `NO_COLOR`), diagnostics mis en forme, journal des actions avec le verbe
//! en évidence, et indicateur de progression pendant les attentes.

use std::io::{IsTerminal, Write};

/// Préfixe d'une ligne de progression : rendue en place sur un terminal
/// (spinner, temps écoulé), une seule fois sinon.
pub const PROGRESS: &str = "\u{1}";

pub struct Ui {
    pub color: bool,
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

    /// Décompte des diagnostics, après leur liste.
    pub fn summary(&mut self, errors: usize, warnings: usize) {
        let mut out = std::io::stderr().lock();
        self.clear_progress(&mut out);
        let e = format!("{errors} erreur{}", if errors > 1 { "s" } else { "" });
        let w = format!("{warnings} avertissement{}", if warnings > 1 { "s" } else { "" });
        let _ = writeln!(out, "{}, {}", if errors > 0 { self.paint("1;31", &e) } else { self.paint("2", &e) }, if warnings > 0 { self.paint("1;33", &w) } else { self.paint("2", &w) });
    }

    /// Le plan en clair : une ligne par ressource, et le coût mensuel.
    pub fn plan_summary(&mut self, plan: &cc_compose_model::plan::Plan) {
        use cc_compose_model::plan::*;
        let mut out = std::io::stdout().lock();
        let catalog = crate::plan::addons::catalog();
        let _ = writeln!(out, "{} {}", self.paint("1", "Projet"), plan.project);
        // Une ligne par ressource, et sous une application les variables
        // dont la valeur est réécrite (FQDN de membre, identifiant d'addon,
        // secret) : ce que le compose déclarait ne sera pas ce que le
        // conteneur reçoit, autant le dire (18/09/2026, cas Vikunja).
        let mut rows: Vec<([String; 3], Vec<String>)> = Vec::new();
        let mut monthly = 0.0f64;
        for r in &plan.resources {
            match r {
                Resource::App(a) => {
                    let kind = match (&a.kind, a.run) {
                        (AppKind::Task, RunWhere::Local) => "tâche (locale)",
                        (AppKind::Task, _) => "tâche",
                        _ => "application",
                    };
                    let image = match &a.source {
                        Source::Synthetic { image, .. } => image.clone(),
                        Source::Context { context, .. } => format!("build {context}"),
                    };
                    let scale = if a.scaling.min_flavor == a.scaling.max_flavor && a.scaling.min_instances == a.scaling.max_instances {
                        format!("{} ×{}", a.scaling.min_flavor, a.scaling.min_instances)
                    } else {
                        format!("{}..{} ×{}..{}", a.scaling.min_flavor, a.scaling.max_flavor, a.scaling.min_instances, a.scaling.max_instances)
                    };
                    let extras: Vec<String> = plan
                        .resources
                        .iter()
                        .filter_map(|x| match x {
                            Resource::Domain(d) if d.app == a.id => Some(d.fqdn.clone()),
                            Resource::Redirection(x) if x.app == a.id => Some(format!("{:?} {} → 4040", x.protocol, x.target_port).to_lowercase()),
                            _ => None,
                        })
                        .collect();
                    let mut detail = format!("{image}, {scale}");
                    if !extras.is_empty() {
                        detail.push_str(&format!(", {}", extras.join(", ")));
                    }
                    let rewritten: Vec<String> = a.env.iter().filter(|e| !e.key.starts_with("CLEVER_COMPOSE_") && e.value.0.iter().any(|seg| matches!(seg, Segment::Ref(_)))).map(|e| format!("{} ← {}", e.key, describe_template(&e.value))).collect();
                    rows.push(([a.service.clone(), kind.into(), detail], rewritten));
                }
                Resource::Addon(a) => {
                    let price = catalog.providers.get(&a.provider).and_then(|p| p.plans.iter().find(|pl| pl.slug == a.plan)).map(|pl| pl.price).unwrap_or(0.0);
                    monthly += price;
                    let engine = a.options.get("documentdb").map(|_| " + DocumentDB").unwrap_or("");
                    let access = if a.private { "network group" } else { "hôte public" };
                    rows.push(([a.service.clone(), "addon".into(), format!("{}{engine} {} plan {} ({price} €/mois HT), {access}", a.provider.trim_end_matches("-addon"), a.version.clone().unwrap_or_default(), a.plan)], Vec::new()));
                }
                Resource::NetworkGroup(ng) => {
                    let members: Vec<String> = plan.resources.iter().filter_map(|x| match x {
                        Resource::Membership(m) if m.group == ng.id => m.member.0.split('/').nth(1).map(str::to_string),
                        _ => None,
                    }).collect();
                    rows.push(([ng.network.clone(), "réseau".into(), format!("network group {} : {}", ng.name, members.join(", "))], Vec::new()));
                }
                _ => {}
            }
        }
        let w0 = rows.iter().map(|(r, _)| r[0].chars().count()).max().unwrap_or(0);
        let w1 = rows.iter().map(|(r, _)| r[1].chars().count()).max().unwrap_or(0);
        for (r, sub) in &rows {
            let _ = writeln!(out, "  {:<w0$}  {:<w1$}  {}", self.paint("1", &r[0]), self.paint("2", &r[1]), r[2], w0 = w0 + if self.color { 8 } else { 0 }, w1 = w1 + if self.color { 8 } else { 0 });
            for line in sub {
                let _ = writeln!(out, "  {:w0$}  {:w1$}    {}", "", "", self.paint("2", line));
            }
        }
        let count = |kind: &str, sing: &str, plur: &str| {
            let n = rows.iter().filter(|(r, _)| r[1].starts_with(kind)).count();
            (n > 0).then(|| format!("{n} {}", if n > 1 { plur } else { sing }))
        };
        let parts: Vec<String> = [count("application", "application", "applications"), count("tâche", "tâche", "tâches"), count("addon", "addon", "addons"), count("réseau", "network group", "network groups")].into_iter().flatten().collect();
        let _ = writeln!(out, "{} {} ; addons : {monthly} €/mois HT (les applications sont facturées à l'usage)", self.paint("1", "Total"), parts.join(", "));
    }

    /// Bilan final.
    pub fn done(&mut self, ok: bool, msg: &str) {
        let mut out = std::io::stderr().lock();
        self.clear_progress(&mut out);
        let _ = writeln!(out, "{} {msg}", if ok { self.paint("1;32", "✔") } else { self.paint("1;31", "✘") });
    }
}

/// Une valeur d'environnement du plan en clair : les littéraux tels quels,
/// chaque référence décrite entre chevrons.
fn describe_template(t: &cc_compose_model::plan::Template) -> String {
    use cc_compose_model::plan::{Reference, Segment};
    t.0.iter()
        .map(|seg| match seg {
            Segment::Literal(l) => l.clone(),
            Segment::Ref(Reference::MemberFqdn { member }) => {
                let (name, net) = member.0.trim_start_matches("member/").split_once('@').unwrap_or((member.0.as_str(), "?"));
                format!("<FQDN de {name} dans le network group {net}>")
            }
            Segment::Ref(Reference::AddonEnv { addon, var }) => format!("<{var} de l'addon {}, expansée au démarrage>", addon.0.trim_start_matches("addon/")),
            Segment::Ref(Reference::Secret { name }) => format!("<secret {name}>"),
        })
        .collect()
}
