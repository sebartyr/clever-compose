//! `ps` et `logs` : lecture de l'état d'un projet déployé, sans plan ni
//! état local (CLAUDE.md §7). Tout part du nom du projet : applications et
//! network groups par tag `compose-project=`, addons par préfixe de nom et
//! fournisseur connu, comme `down`.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::platform::{AddonInfo, AppInfo, LogEvent, LogQuery, LogTarget, NgInfo, Platform};
use super::reconcile::Log;

/// Fournisseurs d'addons que l'outil crée : les seuls candidats à la
/// découverte par nom (les addons n'ont pas de tags).
pub const ADDON_PROVIDERS: &[&str] = &["postgresql-addon", "mysql-addon", "redis-addon", "mongodb-addon", "es-addon"];

/// Une application du projet, avec le service Compose lu dans ses tags.
#[derive(Debug, Clone)]
pub struct ProjectApp {
    pub service: String,
    pub info: AppInfo,
    pub tags: Vec<String>,
}

/// Un addon du projet, le service étant le nom sans le préfixe du projet.
#[derive(Debug, Clone)]
pub struct ProjectAddon {
    pub service: String,
    pub info: AddonInfo,
}

#[derive(Debug, Clone, Default)]
pub struct Inventory {
    pub apps: Vec<ProjectApp>,
    pub addons: Vec<ProjectAddon>,
    pub ngs: Vec<NgInfo>,
}

/// Ce que la plateforme porte pour `project`.
pub async fn discover<P: Platform>(project: &str, platform: &P) -> Result<Inventory, String> {
    let project_tag = format!("compose-project={project}");
    let prefix = format!("{project}-");
    let mut inv = Inventory::default();
    for x in platform.list_apps().await? {
        if !x.name.starts_with(&prefix) {
            continue;
        }
        let tags = platform.get_tags(&x.id).await?;
        if !tags.iter().any(|t| t == &project_tag) {
            continue;
        }
        let service = tags.iter().find_map(|t| t.strip_prefix("compose-service=")).map(str::to_string).unwrap_or_else(|| x.name[prefix.len()..].to_string());
        inv.apps.push(ProjectApp { service, info: x, tags });
    }
    for x in platform.list_addons().await? {
        if x.name.starts_with(&prefix) && ADDON_PROVIDERS.contains(&x.provider.as_str()) {
            inv.addons.push(ProjectAddon { service: x.name[prefix.len()..].to_string(), info: x });
        }
    }
    for x in platform.list_ngs().await? {
        if x.label.starts_with(&prefix) && x.tags.iter().any(|t| t == &project_tag) {
            inv.ngs.push(x);
        }
    }
    inv.apps.sort_by(|a, b| a.service.cmp(&b.service));
    inv.addons.sort_by(|a, b| a.service.cmp(&b.service));
    Ok(inv)
}

/// `clever-compose ps` : une ligne par ressource, à la manière de
/// `docker compose ps` (service, type, identifiant, état, accès).
pub async fn ps<P: Platform>(project: &str, platform: &P, log: &mut Log) -> Result<(), String> {
    let inv = discover(project, platform).await?;
    if inv.apps.is_empty() && inv.addons.is_empty() && inv.ngs.is_empty() {
        log(&format!("aucune ressource pour le projet `{project}`"));
        return Ok(());
    }
    let mut rows: Vec<[String; 5]> = Vec::new();
    for a in &inv.apps {
        let kind = if a.info.instance.lifetime == "TASK" { "tâche" } else { "app" };
        let instances = platform.instances(&a.info.id).await?;
        let last = platform.last_deployment(&a.info.id).await?;
        let state = if !instances.is_empty() {
            let mut counts: Vec<(String, usize)> = Vec::new();
            for i in &instances {
                match counts.iter_mut().find(|(s, _)| *s == i.state) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((i.state.clone(), 1)),
                }
            }
            counts.iter().map(|(s, n)| format!("{n} {}", s.to_lowercase())).collect::<Vec<_>>().join(", ")
        } else {
            match last {
                Some(d) => format!("arrêtée (dernier déploiement : {})", d.state.to_lowercase()),
                None => "jamais déployée".into(),
            }
        };
        // L'accès public : le premier domaine (les tâches n'en ont pas d'utile).
        let access = if kind == "tâche" { String::new() } else { platform.list_domains(&a.info.id).await?.into_iter().next().map(|d| format!("https://{d}")).unwrap_or_default() };
        rows.push([a.service.clone(), kind.into(), a.info.id.clone(), state, access]);
    }
    for a in &inv.addons {
        rows.push([a.service.clone(), "addon".into(), a.info.id.clone(), format!("{} ({})", a.info.provider.trim_end_matches("-addon"), a.info.plan_slug), String::new()]);
    }
    for n in &inv.ngs {
        rows.push([n.label.trim_start_matches(&format!("{project}-")).to_string(), "réseau".into(), n.id.clone(), format!("{} membre(s)", n.members.len()), String::new()]);
    }
    let header = ["SERVICE", "TYPE", "ID", "ÉTAT", "ACCÈS"].map(String::from);
    let mut widths = [0usize; 5];
    for r in std::iter::once(&header).chain(rows.iter()) {
        for (i, c) in r.iter().enumerate() {
            widths[i] = widths[i].max(c.chars().count());
        }
    }
    let fmt = |r: &[String; 5]| r.iter().enumerate().map(|(i, c)| format!("{:<w$}", c, w = widths[i])).collect::<Vec<_>>().join("  ").trim_end().to_string();
    log(&fmt(&header));
    for r in &rows {
        log(&fmt(r));
    }
    Ok(())
}

/// Ce que `logs` reçoit de la ligne de commande.
#[derive(Debug, Clone, Default)]
pub struct LogsOptions {
    /// Services demandés ; vide = tous.
    pub services: Vec<String>,
    /// Début : durée relative (`10m`, `2h`, `1d`) ou horodatage RFC 3339.
    pub since: Option<String>,
    /// Suivre le flux au lieu de s'arrêter à l'instant présent.
    pub follow: bool,
    /// Nombre maximal de lignes par service.
    pub limit: Option<usize>,
}

/// `clever-compose logs` : les logs des applications et addons du projet,
/// préfixés du nom du service, dans l'ordre d'arrivée (un flux par
/// ressource, lus en parallèle).
pub async fn logs<P: Platform>(project: &str, platform: &P, opts: &LogsOptions, log: &mut Log) -> Result<(), String> {
    let inv = discover(project, platform).await?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let since = match &opts.since {
        None => None,
        Some(s) => Some(match parse_duration(s) {
            Some(d) => rfc3339(now.saturating_sub(d.as_secs())),
            None => s.clone(),
        }),
    };
    let query = LogQuery { since, until: if opts.follow { None } else { Some(rfc3339(now)) }, limit: opts.limit };
    let wanted = |service: &str| opts.services.is_empty() || opts.services.iter().any(|s| s == service);
    let mut targets: Vec<(String, String, bool)> = Vec::new();
    for a in &inv.apps {
        if wanted(&a.service) {
            targets.push((a.service.clone(), a.info.id.clone(), true));
        }
    }
    for a in &inv.addons {
        if wanted(&a.service) {
            targets.push((a.service.clone(), a.info.id.clone(), false));
        }
    }
    for s in &opts.services {
        if !targets.iter().any(|(svc, _, _)| svc == s) {
            return Err(format!("`{s}` : aucun service de ce nom dans le projet `{project}` (services : {})", inv.apps.iter().map(|a| a.service.as_str()).chain(inv.addons.iter().map(|a| a.service.as_str())).collect::<Vec<_>>().join(", ")));
        }
    }
    if targets.is_empty() {
        log(&format!("aucune ressource pour le projet `{project}`"));
        return Ok(());
    }
    let width = targets.iter().map(|(s, _, _)| s.chars().count()).max().unwrap_or(0);
    // Une ligne est datée (RFC 3339, comparable en texte) ; en `-f` on
    // affiche à l'arrivée, sinon on trie l'ensemble par date.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<(String, String)>(256);
    let mut handles = Vec::new();
    for (service, id, is_app) in targets {
        let mut stream = platform.logs(if is_app { LogTarget::App(&id) } else { LogTarget::Resource(&id) }, &query).await?;
        let tx = tx.clone();
        handles.push(tokio::spawn(async move {
            loop {
                match stream.next().await {
                    Ok(Some(LogEvent::Line(l))) => {
                        let _ = tx.send((l.date.clone(), format!("{:<w$} | {} {}", service, l.date, l.message, w = width))).await;
                    }
                    Ok(Some(LogEvent::End(_))) | Ok(None) => break,
                    Err(e) => {
                        let _ = tx.send((String::new(), format!("{:<w$} | erreur : {e}", service, w = width))).await;
                        break;
                    }
                }
            }
        }));
    }
    drop(tx);
    let mut buffered: Vec<(String, String)> = Vec::new();
    while let Some((date, line)) = rx.recv().await {
        if opts.follow {
            log(&line);
        } else {
            buffered.push((date, line));
        }
    }
    buffered.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, line) in &buffered {
        log(line);
    }
    for h in handles {
        let _ = h.await;
    }
    Ok(())
}

/// `10s`, `5m`, `2h`, `1d` (un entier et une unité).
pub fn parse_duration(s: &str) -> Option<Duration> {
    let (num, unit) = s.split_at(s.trim_end_matches(|c: char| c.is_ascii_alphabetic()).len());
    let n: u64 = num.parse().ok()?;
    let mult = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => return None,
    };
    Some(Duration::from_secs(n * mult))
}

/// Secondes depuis l'epoch en RFC 3339 UTC (calendrier proleptique,
/// algorithme de Howard Hinnant), sans dépendance.
pub fn rfc3339(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, (rem % 3600) / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_known_dates() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_789_300_000), "2026-09-13T11:46:40Z");
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("90s"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("2h"), Some(Duration::from_secs(7200)));
        assert_eq!(parse_duration("2026-09-13T00:00:00Z"), None);
    }
}
