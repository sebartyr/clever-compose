//! Le planificateur : du projet Compose typé au plan intermédiaire.
//!
//! Dry-run pur : aucune API. Les seules entrées externes sont la
//! configuration des images (via [`images::ImageResolver`]) et le système
//! de fichiers du projet (bind mounts). Chaque clé Compose est soit
//! traduite, soit ignorée avec un avertissement, soit refusée avec une
//! erreur, jamais approximée (CLAUDE.md §13).

pub mod cli;
pub mod env;
pub mod flavors;
pub mod images;
pub mod policy;
pub mod xclever;

use std::collections::BTreeSet;
use std::path::Path as FsPath;

use cc_compose_model::plan::*;
use cc_compose_model::shim::{PlatformPort, ShimConfig, PLATFORM_HTTP_PORT};
use indexmap::IndexMap;

use crate::compose::model::{Project, Service};
use crate::compose::node::Path;
use crate::diagnostic::{Diagnostic, Diagnostics};
use images::ImageResolver;
use xclever::XClever;

pub struct Context<'a> {
    pub images: &'a dyn ImageResolver,
    /// Racine du projet, pour les bind mounts.
    pub project_dir: &'a FsPath,
    /// Arbre canonique sérialisé, pour l'empreinte.
    pub spec_json: &'a serde_json::Value,
    /// Accepte les volumes nommés comme stockage éphémère.
    pub allow_ephemeral: bool,
}

/// Empreinte de la spécification traduite, les chemins absolus du poste
/// ramenés au projet pour qu'elle soit la même partout.
pub fn spec_hash(spec: &serde_json::Value, project_dir: &FsPath) -> String {
    use sha2::{Digest, Sha256};
    fn relativize(v: &serde_json::Value, prefix: &str) -> serde_json::Value {
        match v {
            serde_json::Value::String(s) => serde_json::Value::String(if s.starts_with(prefix) { format!(".{}", &s[prefix.len()..]) } else { s.clone() }),
            serde_json::Value::Array(a) => serde_json::Value::Array(a.iter().map(|x| relativize(x, prefix)).collect()),
            serde_json::Value::Object(m) => serde_json::Value::Object(m.iter().map(|(k, x)| (k.clone(), relativize(x, prefix))).collect()),
            other => other.clone(),
        }
    }
    let prefix = project_dir.to_string_lossy().trim_end_matches('/').to_string();
    let mut h = Sha256::new();
    h.update(serde_json::to_vec(&relativize(spec, &prefix)).unwrap_or_default());
    let out = h.finalize();
    out.iter().take(16).map(|b| format!("{b:02x}")).collect()
}

pub fn plan(project: &Project, ctx: &Context<'_>, diags: &mut Diagnostics) -> Option<Plan> {
    let hash = spec_hash(ctx.spec_json, ctx.project_dir);
    let project_x = XClever::project(project, diags);
    let mut resources: Vec<Resource> = Vec::new();
    let tags = |service: &str| -> Vec<String> {
        vec![format!("compose-project={}", project.name), format!("compose-service={service}"), format!("compose-spec={hash}")]
    };

    for (k, _) in &project.extra {
        if !k.starts_with("x-") {
            diags.push(Diagnostic::error("unsupported-key", format!("clé de premier niveau non supportée : `{k}`")).with_path(&Path::root().child(k)));
        }
    }
    for (name, v) in &project.volumes {
        if v.external == Some(true) {
            diags.push(Diagnostic::error("volume-external", format!("le volume externe `{name}` n'a pas d'équivalent : pas de volume persistant sur le runtime Docker (CLAUDE.md §3)")).with_path(&Path::root().child("volumes").child(name)));
        }
    }
    for (name, n) in &project.networks {
        let p = Path::root().child("networks").child(name);
        policy::network(name, n, &p, diags);
    }

    // Services one-shot : `depends_on: condition: service_completed_successfully`
    // ou `x-clever.task`.
    let mut tasks: BTreeSet<String> = BTreeSet::new();
    for s in project.services.values() {
        for (dep, d) in &s.depends_on {
            if d.condition == "service_completed_successfully" {
                tasks.insert(dep.clone());
            }
        }
    }

    // Réseaux et appartenances.
    let mut memberships: IndexMap<String, Vec<(String, Vec<String>)>> = IndexMap::new(); // réseau -> [(service, alias)]
    for (name, s) in &project.services {
        for (net, cfg) in &s.networks {
            let mut aliases = vec![name.clone()];
            if let Some(c) = cfg {
                aliases.extend(c.aliases.iter().cloned());
            }
            memberships.entry(net.clone()).or_default().push((name.clone(), aliases));
        }
    }
    for net in memberships.keys() {
        resources.push(Resource::NetworkGroup(NetworkGroup {
            id: ResourceId(format!("ng/{net}")),
            network: net.clone(),
            name: format!("{}-{net}", project.name),
            tags: vec![format!("compose-project={}", project.name), format!("compose-network={net}"), format!("compose-spec={hash}")],
        }));
    }

    let mut ok = true;
    let mut apps: Vec<App> = Vec::new();
    for (name, s) in &project.services {
        let p = Path::root().child("services").child(name);
        let x = XClever::service(s, &p, diags);
        let is_task = tasks.contains(name) || x.task == Some(true);
        match plan_service(project, name, s, &p, &x, &project_x, is_task, ctx, &memberships, diags) {
            Some(app) => {
                let mut app = app;
                app.tags = tags(name);
                apps.push(app);
            }
            None => ok = false,
        }
        for (i, fqdn) in x.domains.iter().enumerate() {
            resources.push(Resource::Domain(Domain { id: ResourceId(format!("domain/{name}/{i}")), app: ResourceId(format!("app/{name}")), fqdn: fqdn.clone() }));
        }
    }
    if !ok || diags.has_errors() {
        return None;
    }

    // Ordre topologique des applications selon depends_on.
    let order = topo_order(&apps, diags)?;
    for name in &order {
        let app = apps.iter().find(|a| a.service == *name).unwrap().clone();
        resources.push(Resource::App(app));
    }
    for (net, members) in &memberships {
        for (service, aliases) in members {
            resources.push(Resource::Membership(Membership {
                id: ResourceId(format!("member/{service}@{net}")),
                member: ResourceId(format!("app/{service}")),
                group: ResourceId(format!("ng/{net}")),
                aliases: aliases.clone(),
            }));
        }
    }
    Some(Plan { project: project.name.clone(), spec_hash: hash, resources })
}

#[allow(clippy::too_many_arguments)]
fn plan_service(
    project: &Project,
    name: &str,
    s: &Service,
    p: &Path,
    x: &XClever,
    px: &XClever,
    is_task: bool,
    ctx: &Context<'_>,
    memberships: &IndexMap<String, Vec<(String, Vec<String>)>>,
    diags: &mut Diagnostics,
) -> Option<App> {
    policy::service(name, s, p, diags);

    // Source et configuration de l'image.
    let (source, intrinsics) = match (&s.build, &s.image) {
        (Some(b), _) => {
            let context = b.context.clone();
            let dockerfile = b.dockerfile.clone().unwrap_or_else(|| "Dockerfile".into());
            if b.dockerfile_inline.is_some() {
                diags.push(Diagnostic::error("build-dockerfile-inline", "`build.dockerfile_inline` n'est pas supporté").with_path(&p.child("build").child("dockerfile_inline")));
                return None;
            }
            let intr = match images::from_dockerfile(FsPath::new(&context), &dockerfile, ctx.images) {
                Ok(i) => i,
                Err(e) => {
                    diags.push(Diagnostic::error("image-unresolved", format!("{name} : {e}")).with_path(&p.child("build")));
                    return None;
                }
            };
            (Source::Context { context, dockerfile, files: Vec::new() }, intr)
        }
        (None, Some(image)) => {
            let intr = match ctx.images.resolve(image) {
                Ok(i) => i,
                Err(e) => {
                    diags.push(Diagnostic::error("image-unresolved", format!("{name} : configuration de l'image `{image}` introuvable : {e}")).with_path(&p.child("image")));
                    return None;
                }
            };
            let private_registry = !images::is_public_registry(image);
            (Source::Synthetic { image: image.clone(), private_registry, files: Vec::new() }, intr)
        }
        (None, None) => {
            diags.push(Diagnostic::error("no-image", format!("le service `{name}` n'a ni `image` ni `build`")).with_path(p));
            return None;
        }
    };

    // Bind mounts de fichiers : copiés dans l'image ; volumes nommés :
    // refusés sans --allow-ephemeral (addons en phase 5).
    let mut files: Vec<CopiedFile> = Vec::new();
    for (i, v) in s.volumes.iter().enumerate() {
        let vp = p.child("volumes").index(i);
        match v.kind.as_str() {
            "bind" => {
                let src = v.source.clone().unwrap_or_default();
                let meta = std::fs::metadata(&src);
                match meta {
                    Ok(m) if m.is_file() => files.push(CopiedFile { source: src, target: v.target.clone(), read_only: v.read_only }),
                    Ok(_) => {
                        diags.push(Diagnostic::error("bind-directory", format!("le montage de répertoire `{src}` vers `{}` n'a pas d'équivalent : pas de volume sur le runtime Docker ; un fichier seul serait copié dans l'image", v.target)).with_path(&vp));
                    }
                    Err(_) => {
                        diags.push(Diagnostic::error("bind-missing", format!("le chemin `{src}` monté vers `{}` n'existe pas sur ce poste", v.target)).with_path(&vp));
                    }
                }
            }
            "volume" => {
                if ctx.allow_ephemeral || x.ephemeral == Some(true) {
                    diags.push(Diagnostic::warning("volume-ephemeral", format!("le volume `{}` monté sur `{}` sera éphémère : les données sont perdues à chaque redéploiement", v.source.clone().unwrap_or_default(), v.target)).with_path(&vp));
                } else {
                    diags.push(Diagnostic::error("volume-unsupported", format!("le volume `{}` monté sur `{}` n'a pas d'équivalent : pas de volume persistant sur le runtime Docker ; utilisez un addon managé, ou `--allow-ephemeral` / `x-clever.ephemeral: true` pour accepter la perte des données", v.source.clone().unwrap_or_default(), v.target)).with_path(&vp));
                }
            }
            "tmpfs" => {}
            other => {
                diags.push(Diagnostic::error("volume-type-unsupported", format!("montage de type `{other}` non supporté")).with_path(&vp));
            }
        }
    }
    let source = match source {
        Source::Synthetic { image, private_registry, .. } => Source::Synthetic { image, private_registry, files },
        Source::Context { context, dockerfile, .. } => Source::Context { context, dockerfile, files },
    };

    // Commande : `entrypoint` remplace l'entrypoint de l'image et vide son
    // cmd ; `command` remplace le cmd.
    let (entrypoint, cmd) = match (&s.entrypoint, &s.command) {
        (Some(e), Some(c)) => (e.clone(), c.clone()),
        (Some(e), None) => (e.clone(), Vec::new()),
        (None, Some(c)) => (intrinsics.entrypoint.clone(), c.clone()),
        (None, None) => (intrinsics.entrypoint.clone(), intrinsics.cmd.clone()),
    };
    let mut argv = entrypoint;
    argv.extend(cmd);
    if argv.is_empty() {
        diags.push(Diagnostic::error("no-command", format!("le service `{name}` n'a aucune commande : ni l'image ni le service ne déclarent d'entrypoint ou de commande")).with_path(p));
        return None;
    }
    let user = s.user.clone().or_else(|| intrinsics.user.clone()).filter(|u| !u.is_empty() && u != "root" && u != "0");
    let workdir = s.working_dir.clone().or_else(|| intrinsics.workdir.clone()).filter(|w| !w.is_empty());

    // Port plateforme (phase 0) : 8080 servi par le shim.
    let port = platform_port(name, s, x, &intrinsics, p, is_task, diags);

    // Variables d'environnement : refus des variables réservées, réécriture
    // des noms de services, table DNS.
    let mut env: Vec<EnvVar> = Vec::new();
    for (k, v) in &s.environment {
        let ep = p.child("environment").child(k);
        if k == "CC_DOCKER_EXPOSED_HTTP_PORT" || k == "CC_MOUNT_DOCKER_SOCKET" {
            diags.push(Diagnostic::error("reserved-variable", format!("`{k}` est interdite : elle ferait sortir le conteneur du mode `--net host` et casserait la joignabilité dans le network group (CLAUDE.md §3)")).with_path(&ep));
            continue;
        }
        let Some(v) = v else {
            diags.push(Diagnostic::warning("environment-unset", format!("`{k}` est déclarée sans valeur et absente de l'environnement : ignorée")).with_path(&ep));
            continue;
        };
        env.push(EnvVar { key: k.clone(), value: env::rewrite(k, v, name, s, project, memberships, &ep, diags) });
    }
    let dns_map = env::dns_map(name, s, memberships);
    if !dns_map.0.is_empty() {
        env.push(EnvVar { key: "CC_COMPOSE_DNS_MAP".into(), value: dns_map });
    }
    if let Some(path) = &s.healthcheck.as_ref().and_then(|h| health_path(h, name, p, diags)) {
        env.push(EnvVar { key: "CC_HEALTH_CHECK_PATH".into(), value: Template::literal(path.clone()) });
    }
    env.sort_by(|a, b| a.key.cmp(&b.key));

    let scaling = flavors::scaling(s, x, px, p, diags);
    let region = x.region.clone().or_else(|| px.region.clone()).unwrap_or_else(|| "par".into());

    let mut depends_on: Vec<ResourceId> = Vec::new();
    for (dep, d) in &s.depends_on {
        let dp = p.child("depends_on").child(dep);
        if !project.services.contains_key(dep) {
            diags.push(Diagnostic::error("depends-on-unknown", format!("`{name}` dépend de `{dep}`, qui n'existe pas")).with_path(&dp));
            continue;
        }
        match d.condition.as_str() {
            "service_started" | "service_healthy" | "service_completed_successfully" => {}
            other => diags.push(Diagnostic::warning("depends-on-condition", format!("condition `{other}` inconnue, traitée comme `service_started`")).with_path(&dp)),
        }
        depends_on.push(ResourceId(format!("app/{dep}")));
    }

    Some(App {
        id: ResourceId(format!("app/{name}")),
        service: name.to_string(),
        name: format!("{}-{name}", project.name),
        kind: if is_task { AppKind::Task } else { AppKind::Service },
        region,
        scaling,
        source,
        image: intrinsics,
        shim: ShimConfig { argv, workdir, user, port },
        env,
        depends_on,
        health_check_path: None,
        tags: Vec::new(),
    })
}

/// Ports connus comme non HTTP, pour deviner le mode du port plateforme
/// quand `x-clever` ne le dit pas.
const NON_HTTP_PORTS: &[u16] = &[5432, 3306, 33060, 6379, 27017, 5672, 11211, 9092, 2181, 25, 465, 587, 993, 995, 22, 389, 636, 5433, 1433, 1521, 9000];

fn platform_port(name: &str, s: &Service, x: &XClever, intr: &ImageIntrinsics, p: &Path, is_task: bool, diags: &mut Diagnostics) -> PlatformPort {
    if is_task {
        // Une tâche n'a pas de contrôle de santé HTTP : elle s'arrête.
        return PlatformPort::Health { ready_port: None };
    }
    if let Some(port) = x.http_port {
        return if port == PLATFORM_HTTP_PORT { PlatformPort::Image } else { PlatformPort::Forward { target_port: port } };
    }
    let published: Vec<u16> = s.ports.iter().map(|pt| pt.target).collect();
    let candidates: Vec<u16> = if !published.is_empty() {
        published.clone()
    } else if !s.expose.is_empty() {
        s.expose.iter().filter_map(|e| e.split('/').next().and_then(|x| x.split('-').next()).and_then(|x| x.parse().ok())).collect()
    } else {
        intr.exposed_ports.clone()
    };
    let protocol = x.protocol.clone();
    match (candidates.first(), protocol.as_deref()) {
        (None, _) => {
            diags.push(Diagnostic::warning("worker", format!("`{name}` n'expose aucun port : traité comme un worker, le shim répondra au contrôle de santé sur 8080")).with_path(p));
            PlatformPort::Health { ready_port: None }
        }
        (Some(port), Some("tcp")) => PlatformPort::Health { ready_port: Some(*port) },
        (Some(port), Some("http")) => forward(*port),
        (Some(port), _) => {
            if NON_HTTP_PORTS.contains(port) || s.ports.is_empty() && intr.exposed_ports.iter().any(|e| NON_HTTP_PORTS.contains(e)) {
                diags.push(Diagnostic::warning("port-assumed-tcp", format!("`{name}` : le port {port} est supposé non HTTP ; il sera joignable dans le network group, et le shim répondra au contrôle de santé sur 8080 dès qu'il écoute. `x-clever.protocol: http` pour l'exposer publiquement")).with_path(p));
                PlatformPort::Health { ready_port: Some(*port) }
            } else {
                if candidates.len() > 1 {
                    diags.push(Diagnostic::warning("multiple-ports", format!("`{name}` publie plusieurs ports ({:?}) : seul {port} est exposé publiquement, les autres restent joignables dans le network group", candidates)).with_path(&p.child("ports")));
                }
                if s.ports.is_empty() {
                    diags.push(Diagnostic::warning("port-assumed-http", format!("`{name}` : le port {port} est supposé HTTP et servira le contrôle de santé sur 8080 ; `x-clever.protocol: tcp` s'il ne l'est pas")).with_path(p));
                }
                forward(*port)
            }
        }
    }
}

fn forward(port: u16) -> PlatformPort {
    if port == PLATFORM_HTTP_PORT { PlatformPort::Image } else { PlatformPort::Forward { target_port: port } }
}

/// `CC_HEALTH_CHECK_PATH` depuis un healthcheck HTTP (`curl`/`wget` vers une
/// URL locale) ; sinon avertissement.
fn health_path(h: &crate::compose::model::HealthCheck, name: &str, p: &Path, diags: &mut Diagnostics) -> Option<String> {
    if h.disable == Some(true) || h.test.first().map(String::as_str) == Some("NONE") {
        return None;
    }
    let joined = h.test.join(" ");
    let url = joined.split_whitespace().find(|w| w.starts_with("http://") || w.starts_with("https://")).map(|w| w.trim_matches(|c| c == '"' || c == '\''));
    match url {
        Some(u) => {
            let after = u.splitn(4, '/').nth(3).map(|s| format!("/{s}")).unwrap_or_else(|| "/".into());
            Some(after)
        }
        None => {
            diags.push(Diagnostic::warning("healthcheck-ignored", format!("`{name}` : le healthcheck n'est pas une requête HTTP, il est ignoré (la plateforme ne connaît que `CC_HEALTH_CHECK_PATH`)")).with_path(&p.child("healthcheck")));
            None
        }
    }
}

fn topo_order(apps: &[App], diags: &mut Diagnostics) -> Option<Vec<String>> {
    let mut order: Vec<String> = Vec::new();
    let mut state: IndexMap<String, u8> = apps.iter().map(|a| (a.service.clone(), 0u8)).collect();
    fn visit(name: &str, apps: &[App], state: &mut IndexMap<String, u8>, order: &mut Vec<String>, stack: &mut Vec<String>) -> Result<(), Vec<String>> {
        match state.get(name).copied() {
            Some(2) => return Ok(()),
            Some(1) => {
                stack.push(name.to_string());
                return Err(stack.clone());
            }
            _ => {}
        }
        state.insert(name.to_string(), 1);
        stack.push(name.to_string());
        if let Some(app) = apps.iter().find(|a| a.service == name) {
            for dep in &app.depends_on {
                let dep_name = dep.0.trim_start_matches("app/");
                visit(dep_name, apps, state, order, stack)?;
            }
        }
        stack.pop();
        state.insert(name.to_string(), 2);
        order.push(name.to_string());
        Ok(())
    }
    for a in apps {
        if let Err(cycle) = visit(&a.service, apps, &mut state, &mut order, &mut Vec::new()) {
            diags.push(Diagnostic::error("depends-on-cycle", format!("cycle de dépendances : {}", cycle.join(" -> "))).with_path(&Path::root().child("services")));
            return None;
        }
    }
    Some(order)
}
