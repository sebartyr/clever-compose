//! Réconciliation du plan avec l'état de la plateforme.

use std::collections::BTreeMap;
use std::path::Path as FsPath;
use std::time::Duration;

use cc_compose_model::plan::*;

use super::git;
use super::platform::{EnvVar as PEnv, NewAddon, NewApp, Platform};
use crate::plan::sources;

pub struct Options<'a> {
    pub project_dir: &'a FsPath,
    /// Ne rien modifier : décrire ce qui serait fait.
    pub dry_run: bool,
    /// Ne pas attendre la fin des déploiements.
    pub no_wait: bool,
    /// La substitution par des addons est facturée : elle se confirme.
    pub confirm_addons: bool,
}

/// Journal des actions, lisible par l'utilisateur.
pub type Log = Box<dyn FnMut(&str) + Send>;

pub struct State {
    pub app_ids: BTreeMap<String, String>,   // service -> app id
    pub ng_ids: BTreeMap<String, String>,    // réseau -> ng id
    pub addon_ids: BTreeMap<String, String>, // service -> addon id
}

pub async fn up<P: Platform>(plan: &Plan, platform: &P, opts: &Options<'_>, log: &mut Log) -> Result<State, String> {
    let project_tag = format!("compose-project={}", plan.project);
    let mut state = State { app_ids: BTreeMap::new(), ng_ids: BTreeMap::new(), addon_ids: BTreeMap::new() };

    // 0. Addons managés, par nom ; création confirmée (facturée).
    let addons: Vec<&Addon> = plan.resources.iter().filter_map(|r| if let Resource::Addon(a) = r { Some(a) } else { None }).collect();
    if !addons.is_empty() {
        let existing = platform.list_addons().await?;
        for a in &addons {
            match existing.iter().find(|x| x.name == a.name) {
                Some(x) => {
                    if x.provider != a.provider {
                        return Err(format!("l'addon `{}` ({}) existe avec le fournisseur {}, le plan demande {}", a.name, x.id, x.provider, a.provider));
                    }
                    if x.plan_slug != a.plan {
                        log(&format!("addon {} : existe ({}) avec le plan `{}`, le plan demandé `{}` n'est pas appliqué (changement de plan manuel)", a.name, x.id, x.plan_slug, a.plan));
                    } else {
                        log(&format!("addon {} : existe ({})", a.name, x.id));
                    }
                    state.addon_ids.insert(a.service.clone(), x.id.clone());
                }
                None if opts.dry_run => log(&format!("addon {} : à créer ({} plan {}{})", a.name, a.provider, a.plan, a.version.as_ref().map(|v| format!(" version {v}")).unwrap_or_default())),
                None if !opts.confirm_addons => {
                    return Err(format!("l'addon `{}` ({} plan `{}`) serait créé et facturé : relancez avec `--yes` pour confirmer (coût dans la sortie de `plan`)", a.name, a.provider, a.plan));
                }
                None => {
                    let created = platform.create_addon(&NewAddon { name: a.name.clone(), provider: a.provider.clone(), plan: a.plan.clone(), region: a.region.clone(), version: a.version.clone() }).await?;
                    log(&format!("addon {} : créé ({})", a.name, created.id));
                    state.addon_ids.insert(a.service.clone(), created.id);
                }
            }
        }
    }

    // 1. Network groups, par label.
    let existing_ngs = platform.list_ngs().await?;
    for r in &plan.resources {
        let Resource::NetworkGroup(ng) = r else { continue };
        match existing_ngs.iter().find(|x| x.label == ng.name) {
            Some(x) => {
                log(&format!("network group {} : existe ({})", ng.name, x.id));
                state.ng_ids.insert(ng.network.clone(), x.id.clone());
            }
            None if opts.dry_run => log(&format!("network group {} : à créer", ng.name)),
            None => {
                let created = platform.create_ng(&ng.name, &ng.tags).await?;
                log(&format!("network group {} : créé ({})", ng.name, created.id));
                state.ng_ids.insert(ng.network.clone(), created.id);
            }
        }
    }

    // 2. Applications, retrouvées par nom puis confirmées par leurs tags.
    let existing_apps = platform.list_apps().await?;
    let apps: Vec<&App> = plan.resources.iter().filter_map(|r| if let Resource::App(a) = r { Some(a) } else { None }).collect();
    for app in &apps {
        let found = existing_apps.iter().find(|x| x.name == app.name);
        match found {
            Some(x) => {
                let tags = platform.get_tags(&x.id).await?;
                if !tags.iter().any(|t| t == &project_tag) {
                    return Err(format!("l'application `{}` ({}) existe mais ne porte pas le tag `{project_tag}` : elle n'a pas été créée par clever-compose, refus de la modifier", app.name, x.id));
                }
                log(&format!("application {} : existe ({})", app.name, x.id));
                let i = &x.instance;
                if i.min_flavor.name != app.scaling.flavor || i.max_flavor.name != app.scaling.flavor || i.min_instances != app.scaling.min_instances || i.max_instances != app.scaling.max_instances {
                    if opts.dry_run {
                        log(&format!("  scaling à mettre à jour : {} x{}..{}", app.scaling.flavor, app.scaling.min_instances, app.scaling.max_instances));
                    } else {
                        platform.update_scaling(&x.id, &app.scaling.flavor, app.scaling.min_instances, app.scaling.max_instances).await?;
                        log("  scaling mis à jour");
                    }
                }
                if tags != app.tags && !opts.dry_run {
                    platform.set_tags(&x.id, &app.tags).await?;
                }
                state.app_ids.insert(app.service.clone(), x.id.clone());
            }
            None if opts.dry_run => log(&format!("application {} : à créer", app.name)),
            None => {
                let created = platform
                    .create_app(&NewApp {
                        name: app.name.clone(),
                        region: app.region.clone(),
                        flavor: app.scaling.flavor.clone(),
                        min_instances: app.scaling.min_instances,
                        max_instances: app.scaling.max_instances,
                        task: app.kind == AppKind::Task,
                        description: format!("clever-compose : service `{}` du projet `{}`", app.service, plan.project),
                    })
                    .await?;
                platform.set_tags(&created.id, &app.tags).await?;
                log(&format!("application {} : créée ({})", app.name, created.id));
                state.app_ids.insert(app.service.clone(), created.id);
            }
        }
    }
    if opts.dry_run {
        log("dry-run : liaisons, variables, redirections, domaines et déploiements non simulés au-delà de ce point");
        return Ok(state);
    }

    let mut needs_restart_for_link: Vec<String> = Vec::new();
    // 3. Liaisons (à chaud : pas de redémarrage, phase 0).
    let ngs = platform.list_ngs().await?;
    for r in &plan.resources {
        let Resource::Membership(m) = r else { continue };
        let (Some(service), Some(net)) = (m.member.0.strip_prefix("app/"), m.group.0.strip_prefix("ng/")) else { continue };
        let (Some(app_id), Some(ng_id)) = (state.app_ids.get(service), state.ng_ids.get(net)) else { continue };
        let already = ngs.iter().find(|n| &n.id == ng_id).is_some_and(|n| n.members.iter().any(|mm| &mm.id == app_id));
        if already {
            log(&format!("membre {service}@{net} : déjà lié"));
        } else {
            platform.link_member(ng_id, app_id).await?;
            log(&format!("membre {service}@{net} : lié"));
        }
    }

    // 3b. Liaisons aux addons (injection des variables d'addon).
    for app in &apps {
        let app_id = &state.app_ids[&app.service];
        let wanted: Vec<&str> = app.depends_on.iter().filter_map(|d| d.0.strip_prefix("addon/")).collect();
        if wanted.is_empty() {
            continue;
        }
        let linked = platform.linked_addons(app_id).await?;
        for service in wanted {
            let Some(addon_id) = state.addon_ids.get(service) else { continue };
            if linked.contains(addon_id) {
                log(&format!("application {} : addon {service} déjà lié", app.name));
            } else {
                platform.link_addon(app_id, addon_id).await?;
                log(&format!("application {} : addon {service} lié", app.name));
                needs_restart_for_link.push(app.service.clone());
            }
        }
    }

    // 4. Variables d'environnement : templates résolus, comparés à
    // l'existant ; seul un changement réel redémarre (§7).
    let mut needs_restart: BTreeMap<String, bool> = BTreeMap::new();
    for app in &apps {
        let app_id = &state.app_ids[&app.service];
        let mut wanted: Vec<PEnv> = Vec::new();
        for e in &app.env {
            wanted.push(PEnv { name: e.key.clone(), value: resolve(&e.value, plan, app, &state, opts.project_dir)? });
        }
        if let Source::Context { .. } = app.source {
            wanted.push(PEnv { name: "CC_DOCKERFILE".into(), value: sources::CONTEXT_DOCKERFILE.into() });
        }
        wanted.sort_by(|a, b| a.name.cmp(&b.name));
        let mut current = platform.get_env(app_id).await?;
        current.sort_by(|a, b| a.name.cmp(&b.name));
        if current != wanted {
            platform.set_env(app_id, &wanted).await?;
            log(&format!("application {} : environnement mis à jour ({} variables)", app.name, wanted.len()));
            needs_restart.insert(app.service.clone(), true);
        } else {
            log(&format!("application {} : environnement inchangé", app.name));
        }
    }

    // 5. Redirections et domaines.
    for r in &plan.resources {
        match r {
            Resource::Redirection(x) => {
                let Some(service) = x.app.0.strip_prefix("app/") else { continue };
                let app_id = &state.app_ids[service];
                let existing = match x.protocol {
                    Protocol::Tcp => platform.list_tcp_redirs(app_id).await?,
                    Protocol::Udp => platform.list_udp_redirs(app_id).await?,
                };
                if let Some(e) = existing.iter().find(|e| e.namespace == x.namespace) {
                    log(&format!("redirection {:?} {service} ({}) : existe, port {}", x.protocol, x.namespace, e.port));
                } else {
                    let created = match x.protocol {
                        Protocol::Tcp => platform.add_tcp_redir(app_id, &x.namespace).await?,
                        Protocol::Udp => platform.add_udp_redir(app_id, &x.namespace).await?,
                    };
                    log(&format!("redirection {:?} {service} ({}) : créée, port {}", x.protocol, x.namespace, created.port));
                }
            }
            Resource::Domain(d) => {
                let Some(service) = d.app.0.strip_prefix("app/") else { continue };
                let app_id = &state.app_ids[service];
                if platform.list_domains(app_id).await?.iter().any(|f| f == &d.fqdn) {
                    log(&format!("domaine {} : existe", d.fqdn));
                } else {
                    platform.add_domain(app_id, &d.fqdn).await?;
                    log(&format!("domaine {} : ajouté", d.fqdn));
                }
            }
            _ => {}
        }
    }

    // 6. Dépôts synthétiques et déploiements, dans l'ordre topologique du
    // plan, en attendant chaque succès (depends_on).
    let creds = platform.git_credentials();
    for app in &apps {
        let app_id = &state.app_ids[&app.service];
        let info = platform.get_app(app_id).await?;
        let files = repo_files(app, opts.project_dir)?;
        let work = std::env::temp_dir().join(format!("clever-compose-{}-{}", plan.project, app.service));
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
        let local = git::build(&work, &files)?;
        let remote = git::remote_head(&work, &info.deployment.http_url, &creds)?;
        let pushed = if remote == Some(local) {
            log(&format!("application {} : dépôt inchangé ({}), push sauté", app.name, &local.to_string()[..12]));
            false
        } else {
            git::push(&work, &info.deployment.http_url, &creds)?;
            log(&format!("application {} : dépôt poussé ({})", app.name, &local.to_string()[..12]));
            true
        };
        let _ = std::fs::remove_dir_all(&work);
        let restart = needs_restart.get(&app.service).copied().unwrap_or(false) || needs_restart_for_link.contains(&app.service);
        let never_deployed = info.state == "SHOULD_BE_DOWN" && platform.last_deployment(app_id).await?.is_none();
        if !pushed && (restart || never_deployed) {
            platform.redeploy(app_id).await?;
            log(&format!("application {} : redéploiement demandé", app.name));
        }
        if (pushed || restart || never_deployed) && !opts.no_wait {
            wait_deployment(platform, app_id, &app.name, log).await?;
        }
    }
    Ok(state)
}

fn repo_files(app: &App, project_dir: &FsPath) -> Result<Vec<sources::File>, String> {
    let mut files = sources::files(app, project_dir)?;
    if let Source::Context { context, .. } = &app.source {
        let root = project_dir.join(context.trim_start_matches("./"));
        collect_tree(&root, &root, &mut files)?;
    }
    Ok(files)
}

fn collect_tree(root: &FsPath, dir: &FsPath, out: &mut Vec<sources::File>) -> Result<(), String> {
    let mut entries: Vec<_> = std::fs::read_dir(dir).map_err(|e| format!("{} : {e}", dir.display()))?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name();
        if name == ".git" {
            continue;
        }
        let path = e.path();
        let rel = path.strip_prefix(root).unwrap().to_string_lossy().to_string();
        let ft = e.file_type().map_err(|x| x.to_string())?;
        if ft.is_dir() {
            collect_tree(root, &path, out)?;
        } else if ft.is_file() && !out.iter().any(|f| f.path == rel) {
            #[cfg(unix)]
            let executable = {
                use std::os::unix::fs::PermissionsExt;
                e.metadata().map(|m| m.permissions().mode() & 0o111 != 0).unwrap_or(false)
            };
            #[cfg(not(unix))]
            let executable = false;
            out.push(sources::File { path: rel, content: std::fs::read(&path).map_err(|x| format!("{} : {x}", path.display()))?, executable });
        }
    }
    Ok(())
}

/// Résolution d'un template : FQDN dérivés des ids (phase 0), secrets lus à
/// la source, variables d'addon laissées à l'expansion du shim.
pub fn resolve(t: &Template, plan: &Plan, app: &App, state: &State, project_dir: &FsPath) -> Result<String, String> {
    let mut out = String::new();
    for seg in &t.0 {
        match seg {
            Segment::Literal(s) => out.push_str(s),
            Segment::Ref(Reference::MemberFqdn { member }) => {
                let (service, net) = crate::emit::member_parts(plan, &member.0).ok_or_else(|| format!("membre inconnu {}", member.0))?;
                let app_id = state.app_ids.get(&service).ok_or_else(|| format!("application `{service}` sans id"))?;
                let ng_id = state.ng_ids.get(&net).ok_or_else(|| format!("réseau `{net}` sans id"))?;
                out.push_str(&format!("{app_id}.m.{ng_id}.cc-ng.cloud"));
            }
            Segment::Ref(Reference::AddonEnv { var, .. }) => out.push_str(&format!("${{{var}}}")),
            Segment::Ref(Reference::Secret { name }) => {
                let b = app.secrets.iter().find(|s| &s.name == name).ok_or_else(|| format!("secret `{name}` sans source"))?;
                match &b.source {
                    SecretSource::File { path } => {
                        let p = project_dir.join(path.trim_start_matches("./"));
                        let v = std::fs::read_to_string(&p).map_err(|e| format!("secret `{name}` : {} : {e}", p.display()))?;
                        out.push_str(v.trim_end_matches('\n'));
                    }
                    SecretSource::Environment { var } => {
                        out.push_str(&std::env::var(var).map_err(|_| format!("secret `{name}` : variable {var} absente"))?);
                    }
                }
            }
        }
    }
    Ok(out)
}

async fn wait_deployment<P: Platform>(platform: &P, app_id: &str, name: &str, log: &mut Log) -> Result<(), String> {
    log(&format!("application {name} : attente du déploiement"));
    let started = std::time::Instant::now();
    let mut last_seen: Option<u64> = None;
    loop {
        if started.elapsed() > Duration::from_secs(30 * 60) {
            return Err(format!("application {name} : déploiement trop long (30 min)"));
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
        let Some(d) = platform.last_deployment(app_id).await? else { continue };
        if last_seen != Some(d.id) {
            last_seen = Some(d.id);
        }
        match d.state.as_str() {
            "OK" => {
                log(&format!("application {name} : déployée"));
                return Ok(());
            }
            "FAIL" | "CANCELLED" => return Err(format!("application {name} : déploiement {} ({})", d.state, d.uuid)),
            _ => {}
        }
    }
}
