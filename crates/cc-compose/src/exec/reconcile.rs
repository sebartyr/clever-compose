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
    /// Pousser les assets dans Cellar même si le compose ne le demande pas.
    pub assets: bool,
    /// La substitution par des addons est facturée : elle se confirme.
    pub confirm_addons: bool,
}

/// Journal des actions, lisible par l'utilisateur.
pub type Log = Box<dyn FnMut(&str) + Send>;

pub struct State {
    pub app_ids: BTreeMap<String, String>,   // service -> app id
    pub ng_ids: BTreeMap<String, String>,    // réseau -> ng id
    pub addon_ids: BTreeMap<String, String>, // service -> addon id
    /// service -> id réel de l'addon (membre de network group, FQDN).
    pub addon_real_ids: BTreeMap<String, String>,
}

pub async fn up<P: Platform>(plan: &Plan, platform: &P, opts: &Options<'_>, log: &mut Log) -> Result<State, String> {
    let project_tag = format!("compose-project={}", plan.project);
    let mut state = State { app_ids: BTreeMap::new(), ng_ids: BTreeMap::new(), addon_ids: BTreeMap::new(), addon_real_ids: BTreeMap::new() };

    // 0. Addons managés, par nom ; création confirmée (facturée).
    let addons: Vec<&Addon> = plan.resources.iter().filter_map(|r| if let Resource::Addon(a) = r { Some(a) } else { None }).collect();
    if !addons.is_empty() {
        let existing = platform.list_addons().await?;
        for a in &addons {
            // Le nom n'est pas unique sur la plateforme et les addons n'ont
            // pas de tags : un homonyme du bon fournisseur est le nôtre ;
            // plusieurs, personne ne tranche à la place de l'utilisateur.
            let same: Vec<&super::platform::AddonInfo> = existing.iter().filter(|x| x.name == a.name && x.provider == a.provider).collect();
            if same.len() > 1 {
                return Err(format!("{} addons `{}` ({}) portent ce nom : {} ; supprimez les doublons avant de continuer", same.len(), a.name, a.provider, same.iter().map(|x| x.id.as_str()).collect::<Vec<_>>().join(", ")));
            }
            match same.first().copied().or_else(|| existing.iter().find(|x| x.name == a.name)) {
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
                    state.addon_real_ids.insert(a.service.clone(), x.real_id.clone());
                }
                None if opts.dry_run => log(&format!("addon {} : à créer ({} plan {}{})", a.name, a.provider, a.plan, a.version.as_ref().map(|v| format!(" version {v}")).unwrap_or_default())),
                None if !opts.confirm_addons => {
                    return Err(format!("l'addon `{}` ({} plan `{}`) serait créé et facturé : relancez avec `--yes` pour confirmer (coût dans la sortie de `plan`)", a.name, a.provider, a.plan));
                }
                None => {
                    let created = platform.create_addon(&NewAddon { name: a.name.clone(), provider: a.provider.clone(), plan: a.plan.clone(), region: a.region.clone(), version: a.version.clone(), options: a.options.clone() }).await?;
                    log(&format!("addon {} : créé ({})", a.name, created.id));
                    // L'endpoint DocumentDB apparaît quelques secondes après la
                    // création : lier avant, c'est déployer sans son URI.
                    if a.options.get("documentdb").is_some_and(|v| v == "true") {
                        let started = std::time::Instant::now();
                        loop {
                            if platform.addon_env(&created.id).await?.iter().any(|e| e.name == "DOCUMENTDB_ADDON_URI") {
                                log(&format!("addon {} : endpoint DocumentDB prêt", a.name));
                                break;
                            }
                            if started.elapsed() > Duration::from_secs(180) {
                                return Err(format!("addon {} : l'endpoint DocumentDB (DOCUMENTDB_ADDON_URI) n'est pas apparu en 3 minutes", a.name));
                            }
                            log(&format!("{}addon {} : attente de l'endpoint DocumentDB ({}s)", crate::ui::PROGRESS, a.name, started.elapsed().as_secs()));
                            tokio::time::sleep(Duration::from_secs(3)).await;
                        }
                    }
                    state.addon_ids.insert(a.service.clone(), created.id);
                    state.addon_real_ids.insert(a.service.clone(), created.real_id);
                }
            }
            // Cellar : buckets créés s'ils manquent, avec les clés de l'addon
            // (qui apparaissent quelques secondes après la création).
            if !a.buckets.is_empty() {
                if opts.dry_run {
                    log(&format!("addon {} : bucket(s) {} à créer s'ils manquent", a.name, a.buckets.join(", ")));
                } else if let Some(id) = state.addon_ids.get(&a.service) {
                    let started = std::time::Instant::now();
                    let env = loop {
                        let env = platform.addon_env(id).await?;
                        if env.iter().any(|e| e.name == "CELLAR_ADDON_KEY_SECRET") {
                            break env;
                        }
                        if started.elapsed() > Duration::from_secs(120) {
                            return Err(format!("addon {} : les clés Cellar (CELLAR_ADDON_KEY_*) ne sont pas apparues en 2 minutes", a.name));
                        }
                        log(&format!("{}addon {} : attente des clés Cellar ({}s)", crate::ui::PROGRESS, a.name, started.elapsed().as_secs()));
                        tokio::time::sleep(Duration::from_secs(3)).await;
                    };
                    for b in &a.buckets {
                        let cellar = super::assets::Cellar::from_env(&env, b)?;
                        if cellar.bucket_exists().await? {
                            log(&format!("addon {} : bucket {b} existe", a.name));
                        } else {
                            cellar.create_bucket().await?;
                            log(&format!("addon {} : bucket {b} créé", a.name));
                        }
                    }
                }
            }
        }
    }

    // 1. Network groups, par label.
    let existing_ngs = platform.list_ngs().await?;
    for r in &plan.resources {
        let Resource::NetworkGroup(ng) = r else { continue };
        // Par label et tag `compose-project` ; un homonyme sans le tag est à
        // quelqu'un d'autre, plusieurs avec le tag sont un doublon à trancher.
        let tagged: Vec<&super::platform::NgInfo> = existing_ngs.iter().filter(|x| x.label == ng.name && x.tags.iter().any(|t| t == &project_tag)).collect();
        if tagged.len() > 1 {
            return Err(format!("{} network groups `{}` portent le tag `{project_tag}` : {} ; supprimez les doublons avant de continuer", tagged.len(), ng.name, tagged.iter().map(|x| x.id.as_str()).collect::<Vec<_>>().join(", ")));
        }
        match tagged.first().copied() {
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
    // Les tâches exécutées depuis le poste (`run: local`) ne sont jamais
    // créées sur la plateforme ; elles prennent leur tour à l'étape 6.
    let all_apps: Vec<&App> = plan.resources.iter().filter_map(|r| if let Resource::App(a) = r { Some(a) } else { None }).collect();
    let apps: Vec<&App> = all_apps.iter().copied().filter(|a| a.run == RunWhere::Platform).collect();
    for app in &apps {
        // Le nom n'est pas unique sur la plateforme : parmi les homonymes,
        // seule celle qui porte le tag `compose-project` est la nôtre ; sans
        // aucune, on crée (un homonyme étranger est signalé, pas touché) ;
        // plusieurs, personne ne tranche à la place de l'utilisateur.
        let mut tagged: Vec<(&super::platform::AppInfo, Vec<String>)> = Vec::new();
        let mut foreign: Vec<&str> = Vec::new();
        for x in existing_apps.iter().filter(|x| x.name == app.name) {
            let tags = platform.get_tags(&x.id).await?;
            if tags.iter().any(|t| t == &project_tag) {
                tagged.push((x, tags));
            } else {
                foreign.push(&x.id);
            }
        }
        if tagged.len() > 1 {
            return Err(format!("{} applications `{}` portent le tag `{project_tag}` : {} ; supprimez les doublons avant de continuer", tagged.len(), app.name, tagged.iter().map(|(x, _)| x.id.as_str()).collect::<Vec<_>>().join(", ")));
        }
        if !foreign.is_empty() {
            log(&format!("application {} : {} homonyme(s) sans le tag `{project_tag}` ({}), laissé(s) tels quels", app.name, foreign.len(), foreign.join(", ")));
        }
        match tagged.pop() {
            Some((x, tags)) => {
                log(&format!("application {} : existe ({})", app.name, x.id));
                let i = &x.instance;
                if i.min_flavor.name != app.scaling.min_flavor || i.max_flavor.name != app.scaling.max_flavor || i.min_instances != app.scaling.min_instances || i.max_instances != app.scaling.max_instances {
                    if opts.dry_run {
                        log(&format!("  scaling à mettre à jour : {}..{} x{}..{}", app.scaling.min_flavor, app.scaling.max_flavor, app.scaling.min_instances, app.scaling.max_instances));
                    } else {
                        platform.update_scaling(&x.id, &app.scaling).await?;
                        log("  scaling mis à jour");
                    }
                }
                // Seuls les tags `compose-*` sont à nous : les nôtres
                // périmés (`compose-spec=` d'un ancien plan) partent, ceux
                // posés par l'utilisateur restent (18/09/2026).
                let wanted = merge_tags(&tags, &app.tags);
                if tags != wanted && !opts.dry_run {
                    platform.set_tags(&x.id, &wanted).await?;
                }
                state.app_ids.insert(app.service.clone(), x.id.clone());
            }
            None if opts.dry_run => log(&format!("application {} : à créer", app.name)),
            None => {
                let created = platform
                    .create_app(&NewApp {
                        name: app.name.clone(),
                        region: app.region.clone(),
                        scaling: app.scaling.clone(),
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
        let Some(net) = m.group.0.strip_prefix("ng/") else { continue };
        // Une application par son id, un addon par son id réel (kind `addon`).
        let (service, member_id, kind) = match (m.member.0.strip_prefix("app/"), m.member.0.strip_prefix("addon/")) {
            (Some(s), _) => (s, state.app_ids.get(s), "application"),
            (_, Some(s)) => (s, state.addon_real_ids.get(s), "addon"),
            _ => continue,
        };
        let (Some(member_id), Some(ng_id)) = (member_id, state.ng_ids.get(net)) else { continue };
        let already = ngs.iter().find(|n| &n.id == ng_id).is_some_and(|n| n.members.iter().any(|mm| &mm.id == member_id));
        if already {
            log(&format!("membre {service}@{net} : déjà lié"));
        } else {
            platform.link_member(ng_id, member_id, kind).await?;
            log(&format!("membre {service}@{net} : lié ({kind})"));
        }
        // Un addon n'est joignable qu'une fois son instance dans le groupe
        // (peer) ; un dédié tout juste créé peut mettre plusieurs minutes.
        // Les consommateurs sont déployés après.
        if kind == "addon" && !opts.dry_run {
            let started = std::time::Instant::now();
            let mut waited = false;
            while !platform.ng_members_with_peer(ng_id).await?.iter().any(|m| m == member_id) {
                if started.elapsed() > Duration::from_secs(900) {
                    return Err(format!("membre {service}@{net} : l'addon n'a pas rejoint le groupe (aucun peer) en 15 minutes"));
                }
                log(&format!("{}membre {service}@{net} : attente de l'instance de l'addon dans le groupe ({}s)", crate::ui::PROGRESS, started.elapsed().as_secs()));
                waited = true;
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            if waited {
                log(&format!("membre {service}@{net} : instance présente"));
            }
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
    // La configuration Docker du poste, lue une fois par `up`.
    let docker_auths = std::cell::OnceCell::new();
    for app in &apps {
        let app_id = &state.app_ids[&app.service];
        let mut wanted: Vec<PEnv> = Vec::new();
        for e in &app.env {
            wanted.push(PEnv { name: e.key.clone(), value: resolve(&e.value, plan, app, &state, opts.project_dir)? });
        }
        if let Source::Context { .. } = app.source {
            wanted.push(PEnv { name: "CC_DOCKERFILE".into(), value: sources::CONTEXT_DOCKERFILE.into() });
        }
        // Registre privé : identifiants lus dans la configuration Docker du
        // poste (jamais dans le plan), posés en `CC_DOCKER_LOGIN_*`.
        if let Source::Synthetic { image, private_registry: true, .. } = &app.source {
            let host = crate::plan::registry::registry_host(image);
            match crate::plan::registry::credentials_for(docker_auths.get_or_init(crate::plan::registry::docker_auths), &host) {
                Some((user, password)) => {
                    wanted.push(PEnv { name: "CC_DOCKER_LOGIN_SERVER".into(), value: host });
                    wanted.push(PEnv { name: "CC_DOCKER_LOGIN_USERNAME".into(), value: user });
                    wanted.push(PEnv { name: "CC_DOCKER_LOGIN_PASSWORD".into(), value: password });
                }
                None => log(&format!("application {} : registre privé {host} sans identifiants dans la configuration Docker du poste ; `docker login {host}` d'abord, sinon le build échouera", app.name)),
            }
        }
        let mut current = platform.get_env(app_id).await?;
        current.sort_by(|a, b| a.name.cmp(&b.name));
        let mut wanted = merge_env(&current, wanted);
        wanted.sort_by(|a, b| a.name.cmp(&b.name));
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
    for app in &all_apps {
        if app.run == RunWhere::Local {
            if opts.dry_run {
                log(&format!("tâche {} : à exécuter depuis ce poste", app.name));
            } else {
                super::local::run_task(app, &state, platform, opts.project_dir, log).await?;
            }
            continue;
        }
        let app_id = &state.app_ids[&app.service];
        let info = platform.get_app(app_id).await?;
        // Lecture du contexte, dépôt synthétique et push (libgit2, réseau
        // synchrone) hors du runtime : un serveur reste réactif pendant
        // un `up`. Répertoire de travail propre à cet appel : deux `up` du
        // même projet (deux clients, deux organisations) ne se marchent
        // pas dessus.
        let (project_dir, label, url, creds2) = (opts.project_dir.to_path_buf(), format!("{}-{}", plan.project, app.service), info.deployment.http_url.clone(), creds.clone());
        let app2 = (*app).clone();
        let (local, pushed) = tokio::task::spawn_blocking(move || -> Result<(git2::Oid, bool), String> {
            let files = repo_files(&app2, &project_dir)?;
            let work = unique_work_dir(&label)?;
            let result = (|| {
                let local = git::build(&work, &files)?;
                let remote = git::remote_head(&work, &url, &creds2)?;
                if remote == Some(local) {
                    return Ok((local, false));
                }
                git::push(&work, &url, &creds2)?;
                Ok((local, true))
            })();
            let _ = std::fs::remove_dir_all(&work);
            result
        })
        .await
        .map_err(|e| format!("application {} : {e}", app.name))??;
        if pushed {
            log(&format!("application {} : dépôt poussé ({})", app.name, &local.to_string()[..12]));
        } else {
            log(&format!("application {} : dépôt inchangé ({}), push sauté", app.name, &local.to_string()[..12]));
        }
        let restart = needs_restart.get(&app.service).copied().unwrap_or(false) || needs_restart_for_link.contains(&app.service);
        let last = platform.last_deployment(app_id).await?;
        let never_deployed = info.state == "SHOULD_BE_DOWN" && last.is_none();
        // Un dernier déploiement en échec se retente : la cause est souvent
        // externe (addon pas encore joignable) et rien dans le dépôt n'a changé.
        let failed = last.as_ref().is_some_and(|d| matches!(d.state.as_str(), "FAIL" | "CANCELLED"));
        // Un service arrêté (`stop`) repart : `up` veut le compose en marche.
        // Une tâche, elle, a fini : elle ne se relance que si quelque chose change.
        let stopped = app.kind != AppKind::Task && info.state == "SHOULD_BE_DOWN" && last.is_some() && !failed;
        if !pushed && (restart || never_deployed || failed || stopped) {
            platform.redeploy(app_id).await?;
            log(&format!("application {} : redéploiement demandé{}", app.name, if failed { " (dernier déploiement en échec)" } else if stopped { " (arrêtée)" } else { "" }));
        }
        if (pushed || restart || never_deployed || failed || stopped) && !opts.no_wait {
            wait_deployment(platform, app_id, &app.name, log).await?;
        }
    }
    // Assets : les fichiers du projet et le plan dans le bucket Cellar du
    // projet, pour une relecture depuis la plateforme.
    if plan.assets || opts.assets {
        super::assets::sync(&platform.owner_id(), plan, opts.project_dir, opts, log).await?;
    }
    Ok(state)
}

/// Un répertoire de travail neuf sous `temp_dir()`, jamais partagé :
/// `clever-compose-<quoi>-<aléa>`, voir [`fresh_dir`].
pub fn unique_work_dir(what: &str) -> Result<std::path::PathBuf, String> {
    fresh_dir(&std::env::temp_dir(), "clever-compose-", what)
}

/// Crée `<base>/<prefix><quoi>-<16 hex aléatoires>` en 0700 par un
/// `create_dir` (atomique : échoue si le chemin existe, lien symbolique
/// compris — pas de course sur un `/tmp` partagé) ; nom imprévisible
/// (`getrandom`), quelques tentatives en cas de collision.
pub fn fresh_dir(base: &FsPath, prefix: &str, what: &str) -> Result<std::path::PathBuf, String> {
    let safe: String = what.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' }).collect();
    for _ in 0..8 {
        let mut rnd = [0u8; 8];
        getrandom::fill(&mut rnd).map_err(|e| format!("aléa : {e}"))?;
        let hex: String = rnd.iter().map(|b| format!("{b:02x}")).collect();
        let dir = base.join(format!("{prefix}{safe}-{hex}"));
        let mut b = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            b.mode(0o700);
        }
        match b.create(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("{} : {e}", dir.display())),
        }
    }
    Err(format!("{} : impossible de créer un répertoire neuf", base.display()))
}

/// Même plafond que `sources::MAX_REPO`, appliqué au contexte de build
/// qui s'ajoute aux fichiers copiés : au-delà, c'est un `.dockerignore`
/// qui manque (`node_modules`, artefacts).
pub const MAX_CONTEXT: u64 = sources::MAX_REPO;

pub fn repo_files(app: &App, project_dir: &FsPath) -> Result<Vec<sources::File>, String> {
    let mut files = sources::files(app, project_dir)?;
    if let Source::Context { context, .. } = &app.source {
        let root = project_dir.join(context.trim_start_matches("./"));
        let ignore = DockerIgnore::load(&root)?;
        let mut total: u64 = files.iter().map(|f| f.content.len() as u64).sum();
        collect_tree(&root, &root, &ignore, &mut files, &mut total)?;
    }
    Ok(files)
}

/// `.dockerignore` du contexte : motifs glob (`*`, `**`, `?`), `!` pour
/// réinclure, `#` commentaires ; la dernière règle qui correspond gagne.
/// Toujours ignoré : `.git`.
pub struct DockerIgnore {
    rules: Vec<(globset::GlobMatcher, bool)>,
}

impl DockerIgnore {
    pub fn load(root: &FsPath) -> Result<Self, String> {
        let mut rules = Vec::new();
        if let Ok(text) = std::fs::read_to_string(root.join(".dockerignore")) {
            for line in text.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let (negate, pat) = match line.strip_prefix('!') {
                    Some(p) => (true, p),
                    None => (false, line),
                };
                let pat = pat.trim_start_matches("./").trim_end_matches('/');
                for candidate in [pat.to_string(), format!("{pat}/**")] {
                    let g = globset::GlobBuilder::new(&candidate).literal_separator(true).build().map_err(|e| format!(".dockerignore : `{line}` : {e}"))?;
                    rules.push((g.compile_matcher(), negate));
                }
            }
        }
        Ok(DockerIgnore { rules })
    }

    pub fn ignored(&self, rel: &str) -> bool {
        if rel == ".git" || rel.starts_with(".git/") {
            return true;
        }
        let mut ignored = false;
        for (m, negate) in &self.rules {
            if m.is_match(rel) {
                ignored = !negate;
            }
        }
        ignored
    }
}

fn collect_tree(root: &FsPath, dir: &FsPath, ignore: &DockerIgnore, out: &mut Vec<sources::File>, total: &mut u64) -> Result<(), String> {
    let mut entries: Vec<_> = std::fs::read_dir(dir).map_err(|e| format!("{} : {e}", dir.display()))?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let path = e.path();
        let rel = path.strip_prefix(root).unwrap().to_string_lossy().to_string();
        if ignore.ignored(&rel) {
            continue;
        }
        let ft = e.file_type().map_err(|x| x.to_string())?;
        if ft.is_dir() {
            collect_tree(root, &path, ignore, out, total)?;
        } else if ft.is_file() && !out.iter().any(|f| f.path == rel) {
            *total += e.metadata().map(|m| m.len()).unwrap_or(0);
            if *total > MAX_CONTEXT {
                return Err(format!("contexte de build {} : plus de {} Mio à pousser ; excluez ce qui n'entre pas dans l'image avec un .dockerignore (node_modules, target, données…)", root.display(), MAX_CONTEXT / (1024 * 1024)));
            }
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
                let (kind, service, net) = crate::emit::member_parts(plan, &member.0).ok_or_else(|| format!("membre inconnu {}", member.0))?;
                let member_id = match kind {
                    crate::emit::MemberKind::App => state.app_ids.get(&service).ok_or_else(|| format!("application `{service}` sans id"))?,
                    crate::emit::MemberKind::Addon => state.addon_real_ids.get(&service).ok_or_else(|| format!("addon `{service}` sans id réel"))?,
                };
                let ng_id = state.ng_ids.get(&net).ok_or_else(|| format!("réseau `{net}` sans id"))?;
                out.push_str(&format!("{member_id}.m.{ng_id}.cc-ng.cloud"));
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
    let started = std::time::Instant::now();
    let mut last_seen: Option<u64> = None;
    loop {
        if started.elapsed() > Duration::from_secs(30 * 60) {
            return Err(format!("application {name} : déploiement trop long (30 min)"));
        }
        log(&format!("{}application {name} : attente du déploiement ({}s)", crate::ui::PROGRESS, started.elapsed().as_secs()));
        tokio::time::sleep(Duration::from_secs(2)).await;
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

/// `down` : supprime ce que le plan décrit et que la plateforme porte avec
/// le tag du projet ; les addons (données) seulement avec `confirm`.
/// `stop` : arrête les instances des applications du projet, sans rien
/// supprimer (addons, network groups, applications et leur configuration
/// restent) ; `start` : relance ce qui est arrêté. Les tâches ne sont pas
/// concernées.
pub async fn stop_start<P: Platform>(project: &str, platform: &P, start: bool, log: &mut Log) -> Result<(), String> {
    let inv = super::inspect::discover(project, platform).await?;
    let mut touched = 0;
    for a in &inv.apps {
        if !a.tagged || a.info.instance.lifetime == "TASK" {
            continue;
        }
        let (name, id) = (&a.info.name, &a.info.id);
        if start {
            if a.info.state == "SHOULD_BE_UP" {
                log(&format!("application {name} : déjà en marche"));
                continue;
            }
            platform.redeploy(id).await?;
            log(&format!("application {name} : démarrage demandé"));
            touched += 1;
        } else {
            if a.info.state == "SHOULD_BE_DOWN" {
                log(&format!("application {name} : déjà arrêtée"));
                continue;
            }
            platform.stop(id).await?;
            log(&format!("application {name} : arrêtée (configuration et addons conservés)"));
            touched += 1;
        }
    }
    if start && touched > 0 {
        for a in &inv.apps {
            if a.tagged && a.info.instance.lifetime != "TASK" && a.info.state != "SHOULD_BE_UP" {
                wait_deployment(platform, &a.info.id, &a.info.name, log).await?;
            }
        }
    }
    if inv.apps.iter().all(|a| !a.tagged) {
        log(&format!("aucune application taguée `compose-project={project}`"));
    }
    Ok(())
}

/// Supprime tout ce que `up` a créé pour `project`, sans recalculer de plan :
/// l'état est celui de l'API (CLAUDE.md §7, « pas de fichier d'état local »),
/// découvert comme pour `ps`. Les addons ne partent qu'avec `confirm`.
pub async fn down<P: Platform>(project: &str, platform: &P, dry_run: bool, confirm: bool, log: &mut Log) -> Result<(), String> {
    let inv = super::inspect::discover(project, platform).await?;
    for a in &inv.apps {
        let (name, id) = (&a.info.name, &a.info.id);
        if !a.tagged {
            log(&format!("application {name} ({id}) : sans tag clever-compose, conservée (créée par Terraform ? `terraform destroy`)"));
            continue;
        }
        if dry_run {
            log(&format!("application {name} ({id}) : à supprimer"));
        } else {
            platform.delete_app(id).await?;
            log(&format!("application {name} ({id}) : supprimée"));
        }
    }
    for a in &inv.addons {
        let (name, id) = (&a.info.name, &a.info.id);
        if dry_run {
            log(&format!("addon {name} ({id}) : à supprimer (données perdues)"));
        } else if !confirm {
            log(&format!("addon {name} ({id}) : conservé (données) ; `--yes` pour le supprimer"));
        } else {
            platform.delete_addon(id).await?;
            log(&format!("addon {name} ({id}) : supprimé"));
        }
    }
    for x in &inv.ngs {
        if dry_run {
            log(&format!("network group {} ({}) : à supprimer", x.label, x.id));
        } else {
            platform.delete_ng(&x.id).await?;
            log(&format!("network group {} ({}) : supprimé", x.label, x.id));
        }
    }
    // Les assets du projet dans le bucket configuré, s'il y en a.
    super::assets::remove(&platform.owner_id(), project, dry_run, log).await?;
    if !inv.apps.iter().any(|a| a.tagged) {
        log(&format!("aucune application taguée `compose-project={project}` : rien à faire ?"));
    }
    Ok(())
}

/// Variable qui liste ce que l'outil gère dans l'environnement d'une
/// application, pour distinguer au `up` suivant une variable retirée du
/// compose (à supprimer) d'une variable ajoutée par l'utilisateur dans la
/// console (à garder). Sans état local (CLAUDE.md §6), 18/09/2026.
pub const MANAGED_VAR: &str = "CLEVER_COMPOSE_MANAGED";

/// Environnement visé : celui du plan, plus les variables de `current`
/// que l'outil ne gérait pas au `up` précédent (posées par l'utilisateur),
/// et [`MANAGED_VAR`] qui liste les nôtres.
pub fn merge_env(current: &[PEnv], mut planned: Vec<PEnv>) -> Vec<PEnv> {
    let previously: std::collections::BTreeSet<&str> = current.iter().find(|e| e.name == MANAGED_VAR).map(|e| e.value.split(',').filter(|s| !s.is_empty()).collect()).unwrap_or_default();
    let mut managed: Vec<String> = planned.iter().map(|e| e.name.clone()).collect();
    managed.sort();
    for e in current {
        // Jamais gérée par nous, absente du plan : à l'utilisateur.
        if e.name != MANAGED_VAR && !previously.contains(e.name.as_str()) && !planned.iter().any(|p| p.name == e.name) {
            planned.push(e.clone());
        }
    }
    planned.push(PEnv { name: MANAGED_VAR.into(), value: managed.join(",") });
    planned
}

/// Tags visés pour une application existante : ceux du plan, plus les tags
/// de `current` hors de l'espace `compose-` (posés par l'utilisateur).
pub fn merge_tags(current: &[String], planned: &[String]) -> Vec<String> {
    let mut out: Vec<String> = planned.to_vec();
    for t in current {
        if !t.starts_with("compose-") && !out.contains(t) {
            out.push(t.clone());
        }
    }
    out
}

#[cfg(test)]
mod env_tests {
    use super::*;
    fn v(n: &str, val: &str) -> PEnv {
        PEnv { name: n.into(), value: val.into() }
    }

    #[test]
    fn user_vars_survive_and_removed_managed_vars_go() {
        // Au `up` précédent l'outil gérait A et B ; l'utilisateur a ajouté
        // MINE dans la console ; le compose ne pose plus que A.
        let current = vec![v("A", "1"), v("B", "2"), v("MINE", "x"), v(MANAGED_VAR, "A,B")];
        let out = merge_env(&current, vec![v("A", "1")]);
        let names: Vec<&str> = out.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["A", "MINE", MANAGED_VAR]);
        assert_eq!(out.iter().find(|e| e.name == MANAGED_VAR).unwrap().value, "A");
    }

    #[test]
    fn first_up_without_marker_keeps_everything_foreign() {
        // Application créée avant ce mécanisme : rien n'est supprimé.
        let current = vec![v("OLD", "1")];
        let out = merge_env(&current, vec![v("A", "1")]);
        assert!(out.iter().any(|e| e.name == "OLD"));
    }

    #[test]
    fn plan_value_wins_over_console_edit() {
        let current = vec![v("A", "console"), v(MANAGED_VAR, "A")];
        let out = merge_env(&current, vec![v("A", "plan")]);
        assert_eq!(out.iter().find(|e| e.name == "A").unwrap().value, "plan");
    }
}

#[cfg(test)]
mod tag_tests {
    #[test]
    fn user_tags_survive_and_stale_compose_tags_go() {
        let current = vec!["compose-project=p".to_string(), "compose-spec=old".to_string(), "prod".to_string(), "team=infra".to_string()];
        let planned = vec!["compose-project=p".to_string(), "compose-service=web".to_string(), "compose-spec=new".to_string()];
        let merged = super::merge_tags(&current, &planned);
        assert_eq!(merged, vec!["compose-project=p", "compose-service=web", "compose-spec=new", "prod", "team=infra"]);
    }
}
