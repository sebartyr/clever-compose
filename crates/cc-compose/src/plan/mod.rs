//! Le planificateur : du projet Compose typé au plan intermédiaire.
//!
//! Dry-run pur : aucune API. Les seules entrées externes sont la
//! configuration des images (via [`images::ImageResolver`]) et le système
//! de fichiers du projet (bind mounts). Chaque clé Compose est soit
//! traduite, soit ignorée avec un avertissement, soit refusée avec une
//! erreur, jamais approximée (CLAUDE.md §13).

pub mod addons;
pub mod cli;
pub mod env;
pub mod flavors;
pub mod images;
pub mod initdb;
pub mod policy;
pub mod registry;
pub mod sources;
pub mod xclever;

use std::collections::BTreeSet;
use std::path::Path as FsPath;

use cc_compose_model::plan::*;
use cc_compose_model::shim::{PlatformPort, ShimConfig, TcpRelay, UdpRelay, PLATFORM_HTTP_PORT};
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
    /// Hôtes de registre pour lesquels le poste a des identifiants
    /// (`docker login`) : une image qui en vient est traitée comme privée.
    pub private_registries: &'a [String],
    /// Projet déposé sur un serveur (`Options::confine`) : rien de la
    /// machine qui calcule le plan ne doit y entrer (variables
    /// d'environnement du serveur, identifiants Docker du serveur).
    pub confined: bool,
}

impl Context<'_> {
    /// Chemin affiché relativement au projet, pour des diagnostics portables.
    pub fn display(&self, path: &str) -> String {
        let prefix = self.project_dir.to_string_lossy().trim_end_matches('/').to_string();
        match path.strip_prefix(&prefix) {
            Some(rest) => format!(".{rest}"),
            None => path.to_string(),
        }
    }
}

/// Empreinte de la spécification traduite, les chemins absolus du poste
/// ramenés au projet pour qu'elle soit la même partout.
pub fn spec_hash(spec: &serde_json::Value, project_dir: &FsPath) -> String {
    use sha2::{Digest, Sha256};
    fn relativize(v: &serde_json::Value, prefix: &str) -> serde_json::Value {
        match v {
            serde_json::Value::String(s) => serde_json::Value::String(match s.strip_prefix(prefix) { Some(rest) => format!(".{rest}"), None => s.clone() }),
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

    // Substitution par les addons (§6) : image reconnue, pas de `build:`,
    // pas d'éphémère demandé.
    let mut addons: env::Addons = IndexMap::new();
    let mut addon_infos: IndexMap<String, (addons::Recognized, addons::Declared)> = IndexMap::new();
    for (name, s) in &project.services {
        let p = Path::root().child("services").child(name);
        let x = XClever::service(s, &p, &mut Diagnostics::default());
        // `--allow-ephemeral` ne concerne que les volumes des services non
        // substituables ; seul `x-clever.ephemeral: true` garde une base en
        // conteneur.
        if s.build.is_some() {
            continue;
        }
        let Some(mut rec) = s.image.as_deref().and_then(addons::recognize) else { continue };
        // `mongo` : DocumentDB (PostgreSQL dédié + option) sauf tag 4.x ou
        // `x-clever.engine: mongodb` ; la version du tag est celle de
        // MongoDB, elle ne désigne pas une version de PostgreSQL.
        if rec.provider == "mongodb-addon" {
            let documentdb = match x.engine.as_deref() {
                Some("documentdb") => true,
                Some(_) => false,
                None => addons::documentdb_by_default(&rec),
            };
            if documentdb {
                rec.provider = addons::DOCUMENTDB;
                rec.version = None;
            }
        }
        if x.ephemeral == Some(true) {
            diags.push(Diagnostic::warning("addon-declined", format!("`{name}` ({}) reste un conteneur éphémère (`x-clever.ephemeral: true`) ; un addon `{}` managé est possible", rec.kind, rec.provider)).with_path(&p.child("image")));
            continue;
        }
        // Membre du network group par défaut (décision du 16/09/2026) ;
        // vérifié pour PostgreSQL, MySQL et Redis, pas pour les autres.
        let mut private = x.private.unwrap_or(true);
        if private && !addons::NG_VERIFIED.contains(&rec.provider) {
            if x.private == Some(true) {
                diags.push(Diagnostic::error("addon-private-unverified", format!("`{name}` : le rattachement d'un addon {} à un network group n'a pas été vérifié (port de service dans le groupe inconnu) ; retirez `x-clever.private` pour le joindre par son hôte public", rec.kind)).with_path(&p.child("x-clever").child("private")));
                continue;
            }
            private = false;
        }
        if s.networks.is_empty() {
            private = false;
        }
        let declared = addons::declared(&rec, s);
        addons.insert(name.clone(), env::AddonRef { provider: rec.provider, private });
        addon_infos.insert(name.clone(), (rec, declared));
    }

    // Réseaux et appartenances : les services, et les addons privés (un
    // addon public est hors network group).
    let mut memberships: IndexMap<String, Vec<(String, Vec<String>)>> = IndexMap::new(); // réseau -> [(service, alias)]
    for (name, s) in &project.services {
        if addons.get(name).is_some_and(|a| !a.private) {
            continue;
        }
        for (net, cfg) in &s.networks {
            let mut aliases = vec![name.clone()];
            if let Some(c) = cfg {
                aliases.extend(c.aliases.iter().cloned());
            }
            memberships.entry(net.clone()).or_default().push((name.clone(), aliases));
        }
    }
    // Le découpage en network groups est un choix, pas un défaut (décision
    // du 13/09/2026) : dès que le projet a plusieurs réseaux, il doit être
    // écrit dans `x-clever.network_groups`.
    match project_x.network_groups.as_deref() {
        None if memberships.len() > 1 => {
            diags.push(Diagnostic::error("network-groups-unspecified", format!("le projet a {} réseaux Compose ({}) : déclarez `x-clever.network_groups` au niveau projet — `per-network` (un network group par réseau, même segmentation qu'en local), `single` (un seul groupe `{}-default`, tous les services se joignent) ou `none` (aucun groupe, les services ne se joignent pas entre eux)", memberships.len(), memberships.keys().cloned().collect::<Vec<_>>().join(", "), project.name)).with_path(&Path::root().child("networks").child(memberships.keys().next().unwrap())));
            return None;
        }
        Some("single") if memberships.len() > 1 => {
            let nets: Vec<String> = memberships.keys().cloned().collect();
            let mut merged: Vec<(String, Vec<String>)> = Vec::new();
            for members in memberships.values() {
                for (svc, aliases) in members {
                    match merged.iter_mut().find(|(s, _)| s == svc) {
                        Some((_, a)) => {
                            for al in aliases {
                                if !a.contains(al) {
                                    a.push(al.clone());
                                }
                            }
                        }
                        None => merged.push((svc.clone(), aliases.clone())),
                    }
                }
            }
            memberships = IndexMap::from([("default".to_string(), merged)]);
            diags.push(Diagnostic::warning("network-groups-merged", format!("`x-clever.network_groups: single` : les réseaux {} deviennent un seul network group `{}-default` ; la segmentation entre eux disparaît", nets.join(", "), project.name)).with_path(&Path::root().child("x-clever").child("network_groups")));
        }
        Some("none") => {
            if !memberships.is_empty() {
                diags.push(Diagnostic::warning("network-groups-none", "`x-clever.network_groups: none` : aucun network group ; une référence d'un service à un autre est laissée telle quelle (`service-unreachable`)".to_string()).with_path(&Path::root().child("x-clever").child("network_groups")));
            }
            memberships.clear();
        }
        _ => {}
    }
    for net in memberships.keys() {
        resources.push(Resource::NetworkGroup(NetworkGroup {
            id: ResourceId(format!("ng/{net}")),
            network: net.clone(),
            name: format!("{}-{net}", project.name),
            tags: vec![format!("compose-project={}", project.name), format!("compose-network={net}"), format!("compose-spec={hash}")],
        }));
    }

    let catalog = addons::catalog();
    let mut addon_plans: IndexMap<String, addons::PlanInfo> = IndexMap::new();
    let mut init_tasks: Vec<App> = Vec::new();
    for (name, (rec, declared)) in &addon_infos {
        let s = &project.services[name];
        let p = Path::root().child("services").child(name);
        let x = XClever::service(s, &p, &mut Diagnostics::default());
        let region = x.region.clone().or_else(|| project_x.region.clone()).unwrap_or_else(|| "par".into());
        let platform_provider = addons::platform_provider(rec.provider);
        let Some(provider) = catalog.providers.get(platform_provider) else {
            diags.push(Diagnostic::error("addon-provider-unknown", format!("fournisseur `{}` absent du catalogue embarqué", platform_provider)).with_path(&p));
            continue;
        };
        let private = addons[name].private;
        let documentdb = rec.provider == addons::DOCUMENTDB;
        // DocumentDB n'existe que sur un plan dédié.
        let plan = match addons::choose_plan(provider, &region, x.plan.as_deref(), private || documentdb) {
            Ok(pl) => pl.clone(),
            Err(e) => {
                diags.push(Diagnostic::error("addon-plan", e).with_path(&p.child("x-clever").child("plan")));
                continue;
            }
        };
        if documentdb && addons::is_shared(&plan) {
            let cheapest = addons::choose_plan(provider, &region, None, true).ok();
            diags.push(Diagnostic::error("addon-documentdb-shared", format!("`{name}` : DocumentDB n'existe que sur un plan PostgreSQL dédié, pas sur `{}`{}", plan.slug, cheapest.map(|c| format!(" ; `x-clever.plan: {}` ({} €/mois HT)", c.slug, c.price)).unwrap_or_default())).with_path(&p.child("x-clever").child("plan")));
            continue;
        }
        // Un plan partagé n'a pas de VM, donc pas de peer dans le groupe.
        if private && addons::is_shared(&plan) {
            let cheapest = addons::choose_plan(provider, &region, None, true).ok();
            diags.push(Diagnostic::error("addon-private-shared", format!("`{name}` : le plan `{}` est partagé, sans instance dédiée, donc injoignable par le network group ; choisissez un plan dédié{} ou `x-clever.private: false` pour joindre l'addon par son hôte public", plan.slug, cheapest.map(|c| format!(" (`x-clever.plan: {}`, {} €/mois HT)", c.slug, c.price)).unwrap_or_default())).with_path(&p.child("x-clever").child("plan")));
            continue;
        }
        // Version : celle de x-clever, sinon celle du tag ; jamais un
        // rabattement silencieux (§6.1).
        let version = match (x.version.clone(), rec.version.clone()) {
            (Some(v), _) => Some(v),
            (None, Some(v)) if rec.kind == "mariadb" => {
                diags.push(Diagnostic::error("addon-version-unavailable", format!("`{name}` : MariaDB {v} n'existe pas comme addon ; le fournisseur est MySQL (versions : {}). Choisissez une version avec `x-clever.version`, ou gardez le conteneur avec `x-clever.ephemeral: true`", provider.versions.join(", "))).with_path(&p.child("image")));
                continue;
            }
            (None, Some(v)) => Some(v),
            (None, None) => provider.default_version.clone(),
        };
        // Une version demandée moins précise que celles servies (`8` pour
        // `8.10.1`, `4.0` pour `4.0.3`) désigne la seule qui la prolonge :
        // même version, pas un rabattement. Plusieurs candidates = ambigu.
        let version = match version {
            Some(v) if !provider.versions.is_empty() && !provider.versions.contains(&v) => {
                let candidates: Vec<&String> = provider.versions.iter().filter(|av| av.starts_with(&format!("{v}."))).collect();
                match candidates.as_slice() {
                    [one] => Some((*one).clone()),
                    [] => {
                        diags.push(Diagnostic::error("addon-version-unavailable", format!("`{name}` : version {v} indisponible pour {} (versions : {}) ; `x-clever.version` pour en choisir une", provider.name, provider.versions.join(", "))).with_path(&p.child("image")));
                        continue;
                    }
                    many => {
                        diags.push(Diagnostic::error("addon-version-ambiguous", format!("`{name}` : version {v} ambiguë pour {} ({}) ; précisez-la avec `x-clever.version`", provider.name, many.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "))).with_path(&p.child("image")));
                        continue;
                    }
                }
            }
            other => other,
        };
        if documentdb {
            if let Some(v) = version.as_deref().and_then(|v| v.split('.').next()?.parse::<u32>().ok()).filter(|v| *v < 17) {
                diags.push(Diagnostic::error("addon-documentdb-version", format!("`{name}` : DocumentDB demande PostgreSQL 17 ou plus, pas {v} (`x-clever.version` est ici la version de PostgreSQL)")).with_path(&p.child("x-clever").child("version")));
                continue;
            }
        }
        // Un plan partagé (`dev`) sert la version de son cluster, quoi qu'on
        // demande (constaté le 16/09/2026 : 16 demandé, 15 servi) : jamais en
        // silence (§6.1).
        let version = if addons::is_shared(&plan) {
            match provider.shared_versions.get(&region) {
                Some(served) => {
                    let asked = x.version.clone().or_else(|| rec.version.clone());
                    if let Some(a) = asked.filter(|a| !served.starts_with(a.as_str())) {
                        let dedicated = addons::choose_plan(provider, &region, None, true).ok();
                        diags.push(Diagnostic::error("addon-version-shared", format!("`{name}` : le plan `{}` est partagé et sert {} {served} dans `{region}`, pas {a} ; `x-clever.version: {served}` pour l'accepter, ou un plan dédié pour {a}{}", plan.slug, provider.name, dedicated.map(|p| format!(" (`x-clever.plan: {}`, {} €/mois HT)", p.slug, p.price)).unwrap_or_default())).with_path(&p.child("image")));
                        continue;
                    }
                    Some(served.clone())
                }
                None => version,
            }
        } else {
            version
        };
        // Coût annoncé (§6) : la substitution est facturée.
        let access = if private { format!(", membre du network group (joint sur le port {}, `x-clever.private: false` pour l'hôte public)", addon_ng_port(rec.provider)) } else { ", joint par son hôte public".to_string() };
        let engine = if documentdb { " avec l'option DocumentDB (API MongoDB 7.0, pilotes 5.0+, TLS imposé ; `x-clever.engine: mongodb` pour l'addon MongoDB 4.0.3)" } else { "" };
        let shown_version = if documentdb { version.clone().unwrap_or_default() } else { version.clone().unwrap_or_else(|| "latest".into()) };
        diags.push(Diagnostic::warning("addon-substitution", format!("`{name}` ({}) devient un addon {} {shown_version}{engine} plan `{}` ({}) : {} €/mois HT{}{access}. Confirmation requise au `up` (`--yes`) ; `x-clever.plan` pour changer de plan, `x-clever.ephemeral: true` pour garder un conteneur sans persistance", s.image.as_deref().unwrap_or(rec.kind), provider.name, plan.slug, plan.name, plan.price, plan.features.get("Max connection limit").map(|c| format!(", {c} connexions max")).unwrap_or_default())).with_path(&p.child("image")));
        // Falaises sémantiques (§6.4).
        if let Some(cmd) = &s.command {
            // Redis déclare son mot de passe par `--requirepass` : ce n'est
            // pas un réglage. Le reste de la commande en est un.
            let mut rest: Vec<&str> = cmd.iter().map(String::as_str).collect();
            if rec.provider == "redis-addon" {
                if rest.first().is_some_and(|a| a.ends_with("redis-server")) {
                    rest.remove(0);
                }
                if let Some(i) = rest.iter().position(|a| *a == "--requirepass") {
                    rest.drain(i..(i + 2).min(rest.len()));
                }
            }
            if !rest.is_empty() {
                diags.push(Diagnostic::error("addon-tuning-args", format!("`{name}` : `command: {}` n'a pas d'équivalent sur un addon managé (pas d'arguments de réglage) ; retirez-la ou gardez un conteneur", cmd.join(" "))).with_path(&p.child("command")));
            }
        }
        for k in s.environment.keys() {
            if k.contains("MULTIPLE_DATABASES") {
                diags.push(Diagnostic::error("addon-multiple-databases", format!("`{name}` : `{k}` demande plusieurs bases dans une instance ; un addon n'en sert qu'une. Déclarez un service par base")).with_path(&p.child("environment").child(k)));
            }
        }
        // Scripts d'initialisation (§6.3) : rejoués par une tâche.
        let display = |path: &str| ctx.display(path);
        match initdb::collect(s, rec, ctx.project_dir, &display) {
            Err(e) => diags.push(Diagnostic::error("addon-init-scripts", format!("`{name}` : {e}")).with_path(&p.child("volumes"))),
            Ok(scripts) if scripts.is_empty() => {}
            Ok(scripts) => match initdb::runner(rec, version.as_deref()) {
                None => diags.push(Diagnostic::error("addon-init-scripts", format!("`{name}` : scripts d'initialisation non supportés pour {}", rec.provider)).with_path(&p.child("volumes"))),
                Some((image, script)) => match (initdb::hash(&scripts, ctx.project_dir), ctx.images.resolve(&image)) {
                    (Err(e), _) => diags.push(Diagnostic::error("addon-init-scripts", format!("`{name}` : {e}")).with_path(&p.child("volumes"))),
                    (_, Err(e)) => diags.push(Diagnostic::error("image-unresolved", format!("{name}-init : image `{image}` : {e}")).with_path(&p.child("volumes"))),
                    (Ok(h), Ok(intr)) => {
                        diags.push(Diagnostic::warning("addon-init-task", format!("`{name}` : {} script(s) d'initialisation rejoués contre l'addon par la tâche `{name}-init` (une fois par contenu, table témoin `cc_compose_init`) : {}", scripts.len(), scripts.iter().map(|x| x.name.as_str()).collect::<Vec<_>>().join(", "))).with_path(&p.child("volumes")));
                        let mut t = initdb::task(&project.name, name, rec, version.as_deref(), &scripts, &h, &region, tags(&format!("{name}-init")), intr, image, script);
                        if x.init.as_deref() == Some("local") {
                            t.run = RunWhere::Local;
                            diags.push(Diagnostic::warning("addon-init-local", format!("`{name}` : `x-clever.init: local` : les scripts sont exécutés depuis ce poste au `up` (Docker s'il est présent, sinon le client `{}` du poste) ; l'export Terraform garde la tâche", initdb::client(rec.provider))).with_path(&p.child("x-clever").child("init")));
                        }
                        init_tasks.push(t);
                    }
                },
            },
        }
        if declared.root_password.is_some() && declared.user.is_none() {
            diags.push(Diagnostic::warning("addon-no-superuser", format!("`{name}` : seul un mot de passe root est déclaré ; l'addon fournit un utilisateur sans superuser (pas de `CREATE EXTENSION` non autorisée, pas de création de base au démarrage)")).with_path(&p));
        }
        addon_plans.insert(name.clone(), plan.clone());
        resources.push(Resource::Addon(Addon {
            id: ResourceId(format!("addon/{name}")),
            service: name.clone(),
            name: format!("{}-{name}", project.name),
            provider: platform_provider.to_string(),
            plan: plan.slug.clone(),
            region,
            version,
            tags: tags(name),
            private,
            options: if documentdb { std::collections::BTreeMap::from([("documentdb".to_string(), "true".to_string())]) } else { Default::default() },
        }));
    }
    if diags.has_errors() {
        return None;
    }

    let mut ok = true;
    let mut apps: Vec<App> = Vec::new();
    let init_task_of: IndexMap<String, String> = init_tasks.iter().map(|t| (t.service.trim_end_matches("-init").to_string(), t.service.clone())).collect();
    apps.extend(init_tasks);
    for (name, s) in &project.services {
        if addons.contains_key(name) {
            continue;
        }
        let p = Path::root().child("services").child(name);
        let x = XClever::service(s, &p, diags);
        let is_task = tasks.contains(name) || x.task == Some(true);
        match plan_service(project, name, s, &p, &x, &project_x, is_task, ctx, &memberships, &addons, &addon_infos, &addon_plans, diags) {
            Some((app, redirections)) => {
                let mut app = app;
                app.tags = tags(name);
                // Un consommateur attend la tâche d'initialisation de l'addon.
                let extra: Vec<ResourceId> = app.depends_on.iter().filter_map(|d| d.0.strip_prefix("addon/")).filter_map(|a| init_task_of.get(a)).map(|t| ResourceId(format!("app/{t}"))).collect();
                app.depends_on.extend(extra);
                apps.push(app);
                resources.extend(redirections.into_iter().map(Resource::Redirection));
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

    // Références mutuelles par l'environnement sans `depends_on` : le graphe
    // des ressources aurait un cycle (export Terraform impossible en une
    // passe). Signalé, et l'arête sans depends_on est conservée : c'est à
    // l'utilisateur de fixer le sens.
    for a in &apps {
        for dep in &a.depends_on {
            let other = dep.0.trim_start_matches("app/");
            if let Some(b) = apps.iter().find(|b| b.service == other) {
                if b.env.iter().any(|e| e.value.0.iter().any(|seg| matches!(seg, Segment::Ref(Reference::MemberFqdn { member }) if member.0 == format!("member/{}@", a.service) || member.0.starts_with(&format!("member/{}@", a.service))))) && !b.depends_on.contains(&a.id) {
                    diags.push(Diagnostic::warning("mutual-reference", format!("`{}` dépend de `{}` qui référence `{}` dans son environnement : références mutuelles, l'export Terraform aura un cycle ; retirez une des deux références ou déclarez le sens par `depends_on`", a.service, other, a.service)).with_path(&Path::root().child("services").child(other).child("environment")));
                }
            }
        }
    }
    // Ordre topologique des applications selon depends_on.
    let order = topo_order(&apps, diags)?;
    for name in &order {
        if let Some(app) = apps.iter().find(|a| a.service == *name) {
            resources.push(Resource::App(app.clone()));
        }
    }
    for (net, members) in &memberships {
        for (service, aliases) in members {
            let kind = if addons.contains_key(service) { "addon" } else { "app" };
            resources.push(Resource::Membership(Membership {
                id: ResourceId(format!("member/{service}@{net}")),
                member: ResourceId(format!("{kind}/{service}")),
                group: ResourceId(format!("ng/{net}")),
                aliases: aliases.clone(),
            }));
        }
    }
    Some(Plan { project: project.name.clone(), spec_hash: hash, resources, assets: project_x.assets.unwrap_or(false) })
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
    addons: &env::Addons,
    addon_infos: &IndexMap<String, (addons::Recognized, addons::Declared)>,
    addon_plans: &IndexMap<String, addons::PlanInfo>,
    diags: &mut Diagnostics,
) -> Option<(App, Vec<Redirection>)> {
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
                    diags.push(Diagnostic::error("image-unresolved", format!("{name} : {}", ctx.display(&e))).with_path(&p.child("build")));
                    return None;
                }
            };
            (Source::Context { context: ctx.display(&context), dockerfile, files: Vec::new() }, intr)
        }
        (None, Some(image)) => {
            let intr = match ctx.images.resolve(image) {
                Ok(i) => i,
                Err(e) => {
                    diags.push(Diagnostic::error("image-unresolved", format!("{name} : configuration de l'image `{image}` introuvable : {e}")).with_path(&p.child("image")));
                    return None;
                }
            };
            // Registre privé = le poste a des identifiants pour cet hôte (une
            // image privée peut vivre sur ghcr.io comme sur un registre interne).
            let host = registry::registry_host(image);
            let private_registry = ctx.private_registries.contains(&host);
            (Source::Synthetic { image: image.clone(), private_registry, files: Vec::new(), generated: Vec::new() }, intr)
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
                let shown = ctx.display(&src);
                let meta = std::fs::metadata(&src);
                match meta {
                    Ok(m) if m.is_file() => files.push(CopiedFile { source: shown.clone(), target: v.target.clone(), read_only: v.read_only }),
                    Ok(_) if ctx.allow_ephemeral || x.ephemeral == Some(true) => {
                        diags.push(Diagnostic::warning("volume-ephemeral", format!("le répertoire `{shown}` monté sur `{}` sera éphémère : rien n'est copié dans l'image et les données sont perdues à chaque redéploiement", v.target)).with_path(&vp));
                    }
                    Ok(_) => {
                        diags.push(Diagnostic::error("bind-directory", format!("le montage de répertoire `{shown}` vers `{}` n'a pas d'équivalent : pas de volume sur le runtime Docker ; un fichier seul serait copié dans l'image. Utilisez un stockage externe (addon, Cellar), ou `--allow-ephemeral` / `x-clever.ephemeral: true` pour accepter la perte des données", v.target)).with_path(&vp));
                    }
                    Err(_) => {
                        diags.push(Diagnostic::error("bind-missing", format!("le chemin `{shown}` monté vers `{}` n'existe pas sur ce poste", v.target)).with_path(&vp));
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
        Source::Synthetic { image, private_registry, generated, .. } => Source::Synthetic { image, private_registry, files, generated },
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

    // Port plateforme (phase 0) : 8080 servi par le shim ; les autres ports
    // publiés deviennent des redirections TCP/UDP, le TCP entrant par 4040.
    let port = platform_port(name, s, x, &intrinsics, p, is_task, diags);
    let (redirections, tcp, udp) = redirections(name, s, x, &port, p, diags);

    // Variables d'environnement : refus des variables réservées, réécriture
    // des noms de services, table DNS limitée aux services référencés.
    let mut referenced: BTreeSet<String> = s.depends_on.keys().cloned().collect();
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
        let mut t = env::rewrite(k, v, name, s, project, memberships, addons, &ep, diags, &mut referenced);
        // Variables discrètes : corrélation par égalité de valeur avec ce que
        // déclare le service substitué (§6.2, cas 2), la clé levant les
        // ambiguïtés.
        if let Some(lit) = t.as_literal() {
            if let Some((addon_name, role)) = correlate(k, &lit, addon_infos) {
                let provider = addon_infos[&addon_name].0.provider;
                // Addon privé atteint par un réseau commun : hôte et port du
                // membre ; les autres rôles restent ceux de la liaison.
                let private_net = addons.get(&addon_name).filter(|a| a.private).and_then(|_| env::shared_net(&addon_name, s, memberships));
                let seg = match (role, &private_net) {
                    (addons::Role::Host, Some(net)) => {
                        // Sans variable de port corrélée, l'application supposera
                        // le port par défaut du protocole, pas 8080.
                        let has_port = s.environment.iter().any(|(k2, v2)| k2 != k && v2.as_deref().is_some_and(|v2| correlate(k2, v2, addon_infos).is_some_and(|(a2, r2)| a2 == addon_name && r2 == addons::Role::Port)));
                        if !has_port {
                            let port = addon_ng_port(provider);
                            diags.push(Diagnostic::warning("addon-private-port", format!("`{k}` reçoit le FQDN de `{addon_name}` dans le network group, où l'addon écoute sur {port} et non sur son port par défaut ; aucune variable de port n'est corrélée : donnez le port à l'application (`{k}=…:{port}` si elle l'accepte, ou une variable de port), ou `x-clever.private: false`")).with_path(&ep));
                        }
                        Some(Segment::Ref(Reference::MemberFqdn { member: ResourceId(format!("member/{addon_name}@{net}")) }))
                    }
                    (addons::Role::Port, Some(_)) => Some(Segment::Literal(addon_ng_port(provider).to_string())),
                    _ => addons::seg(&addon_name, provider, role),
                };
                if let Some(seg) = seg {
                    // L'URI DocumentDB doit porter les paramètres TLS.
                    t = if provider == addons::DOCUMENTDB && role == addons::Role::Uri { Template(vec![seg, Segment::Literal(format!("?{}", addons::DOCUMENTDB_URI_PARAMS))]) } else { Template(vec![seg]) };
                    referenced.insert(addon_name);
                }
            }
        }
        env.push(EnvVar { key: k.clone(), value: t });
    }
    // Variables à expanser par le shim au démarrage (références d'addon).
    let expand: Vec<String> = env.iter().filter(|e| e.value.0.iter().any(|seg| matches!(seg, Segment::Ref(Reference::AddonEnv { .. })))).map(|e| e.key.clone()).collect();
    if !expand.is_empty() {
        env.push(EnvVar { key: "CLEVER_COMPOSE_EXPAND".into(), value: Template::literal(expand.join(",")) });
    }
    // Avertissement connexions (§6.4) : instances × pool contre la limite du plan.
    for (addon_name, plan) in addon_plans {
        if referenced.contains(addon_name) {
            if let Some(max) = addons::max_connections(plan) {
                let replicas = s.deploy.as_ref().and_then(|d| d.replicas).or(s.scale).unwrap_or(1).max(1) as u32;
                if replicas > 1 && replicas * 10 > max {
                    diags.push(Diagnostic::warning("addon-connections", format!("`{name}` : {replicas} instances contre {max} connexions max sur le plan `{}` de `{addon_name}` ; un pool par instance dépasse vite la limite", plan.slug)).with_path(&p.child("deploy")));
                }
            }
        }
    }
    // Secrets : une variable par secret, le fichier écrit par le shim.
    let mut secrets: Vec<SecretBinding> = Vec::new();
    let mut shim_secrets: Vec<cc_compose_model::shim::SecretFile> = Vec::new();
    for (i, sm) in s.secrets.iter().enumerate() {
        let sp = p.child("secrets").index(i);
        let Some(def) = project.secrets.get(&sm.source) else {
            diags.push(Diagnostic::error("secret-unknown", format!("le secret `{}` n'est pas déclaré au premier niveau", sm.source)).with_path(&sp));
            continue;
        };
        let source = if def.external == Some(true) {
            diags.push(Diagnostic::error("secret-external", format!("le secret externe `{}` n'a pas d'équivalent : déclarez-le avec `file:` ou `environment:`", sm.source)).with_path(&Path::root().child("secrets").child(&sm.source)));
            continue;
        } else if let Some(f) = &def.file {
            if !FsPath::new(f).is_file() {
                diags.push(Diagnostic::error("secret-file-missing", format!("le fichier du secret `{}` n'existe pas : {}", sm.source, ctx.display(f))).with_path(&Path::root().child("secrets").child(&sm.source).child("file")));
                continue;
            }
            SecretSource::File { path: ctx.display(f) }
        } else if let Some(var) = &def.environment {
            if ctx.confined {
                // La valeur serait lue dans l'environnement du serveur, pas
                // du poste de l'utilisateur.
                diags.push(Diagnostic::error("secret-environment-confined", format!("le secret `{}` lit la variable `{var}` de l'environnement : sur un projet déposé, elle viendrait du serveur ; déclarez-le avec `file:`", sm.source)).with_path(&Path::root().child("secrets").child(&sm.source).child("environment")));
                continue;
            }
            SecretSource::Environment { var: var.clone() }
        } else {
            diags.push(Diagnostic::error("secret-invalid", format!("le secret `{}` n'a ni `file` ni `environment`", sm.source)).with_path(&Path::root().child("secrets").child(&sm.source)));
            continue;
        };
        let env_name = format!("CLEVER_COMPOSE_SECRET_{}", sm.source.to_ascii_uppercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect::<String>());
        let target = match &sm.target {
            Some(t) if t.starts_with('/') => t.clone(),
            Some(t) => format!("/run/secrets/{t}"),
            None => format!("/run/secrets/{}", sm.source),
        };
        let mode = sm.mode.as_ref().and_then(|m| u32::from_str_radix(m, 8).ok());
        let uid = sm.uid.as_ref().and_then(|u| u.parse().ok());
        let gid = sm.gid.as_ref().and_then(|g| g.parse().ok());
        if (sm.uid.is_some() && uid.is_none()) || (sm.gid.is_some() && gid.is_none()) {
            diags.push(Diagnostic::warning("secret-owner-ignored", format!("`uid`/`gid` du secret `{}` doivent être numériques ; ignorés", sm.source)).with_path(&sp));
        }
        env.push(EnvVar { key: env_name.clone(), value: Template(vec![Segment::Ref(Reference::Secret { name: sm.source.clone() })]) });
        shim_secrets.push(cc_compose_model::shim::SecretFile { env: env_name.clone(), target: target.clone(), mode, uid, gid });
        secrets.push(SecretBinding { name: sm.source.clone(), source, env: env_name, target, mode, uid, gid });
    }
    let dns_map = env::dns_map(name, s, memberships, &referenced, addons);
    if !dns_map.0.is_empty() {
        env.push(EnvVar { key: "CLEVER_COMPOSE_DNS_MAP".into(), value: dns_map.clone() });
    }
    let health_check_path = s.healthcheck.as_ref().and_then(|h| health_path(h, name, p, diags));
    if let Some(path) = &health_check_path {
        env.push(EnvVar { key: "CC_HEALTH_CHECK_PATH".into(), value: Template::literal(path.clone()) });
    }
    // Diagnostic obligatoire sur les proxys (CLAUDE.md §7) : nginx, haproxy
    // et traefik résolvent les noms une fois au démarrage.
    if !dns_map.0.is_empty() {
        let image_name = s.image.as_deref().unwrap_or("").rsplit('/').next().unwrap_or("").split(':').next().unwrap_or("").to_string();
        if ["nginx", "haproxy", "traefik", "openresty"].contains(&image_name.as_str()) {
            diags.push(Diagnostic::warning("proxy-resolver", format!("`{name}` ({image_name}) résout les noms de services une seule fois au démarrage : après un redéploiement d'un service amont, il renverra des 502. Déclarez `resolver 127.0.0.153 valid=30s;` (le stub DNS du shim) et passez l'hôte amont par une variable (`set $upstream http://app:8000; proxy_pass $upstream;`) pour forcer la résolution à chaque requête")).with_path(&p.child("image")));
        }
    }
    env.sort_by(|a, b| a.key.cmp(&b.key));

    let scaling = flavors::scaling(s, x, px, p, diags);
    let region = x.region.clone().or_else(|| px.region.clone()).unwrap_or_else(|| "par".into());

    let mut depends_on: Vec<ResourceId> = Vec::new();
    // Les addons référencés sont des dépendances (liés avant le déploiement).
    for a in &referenced {
        if addons.contains_key(a) {
            depends_on.push(ResourceId(format!("addon/{a}")));
        }
    }
    for (dep, d) in &s.depends_on {
        let dp = p.child("depends_on").child(dep);
        if addons.contains_key(dep) {
            if !depends_on.iter().any(|x| x.0 == format!("addon/{dep}")) {
                depends_on.push(ResourceId(format!("addon/{dep}")));
            }
            continue;
        }
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

    Some((App {
        id: ResourceId(format!("app/{name}")),
        service: name.to_string(),
        name: format!("{}-{name}", project.name),
        kind: if is_task { AppKind::Task } else { AppKind::Service },
        region,
        scaling,
        source,
        image: intrinsics,
        shim: ShimConfig { argv, workdir, user, port, tcp, udp, secrets: shim_secrets },
        env,
        depends_on,
        health_check_path,
        secrets,
        tags: Vec::new(),
        run: RunWhere::Platform,
    }, redirections))
}

/// Ports publiés qui ne sont pas le port HTTP : une redirection chacun. Le
/// TCP et l'UDP entrent tous deux par 4040, donc un seul port de chaque
/// protocole peut être relayé par application.
fn redirections(name: &str, s: &Service, x: &XClever, port: &PlatformPort, p: &Path, diags: &mut Diagnostics) -> (Vec<Redirection>, Option<TcpRelay>, Option<UdpRelay>) {
    let http_port = match port {
        PlatformPort::Image => Some(PLATFORM_HTTP_PORT),
        PlatformPort::Forward { target_port } => Some(*target_port),
        PlatformPort::Health { .. } => None,
    };
    let namespace = x.tcp_namespace.clone().unwrap_or_else(|| "default".into());
    let mut out = Vec::new();
    let mut tcp: Option<TcpRelay> = None;
    let mut udp: Option<UdpRelay> = None;
    let mut seen: Vec<(u16, &str)> = Vec::new();
    for (i, pt) in s.ports.iter().enumerate() {
        if pt.published.is_none() || Some(pt.target) == http_port && pt.protocol == "tcp" {
            continue;
        }
        if seen.contains(&(pt.target, pt.protocol.as_str())) {
            continue;
        }
        seen.push((pt.target, &pt.protocol));
        let pp = p.child("ports").index(i);
        let protocol = match pt.protocol.as_str() {
            "udp" => {
                if udp.is_none() {
                    udp = Some(UdpRelay { target_port: pt.target });
                    diags.push(Diagnostic::warning("redirection-udp", format!("`{name}` : le port UDP {} publié devient une redirection UDP (port externe attribué par la plateforme), relayée depuis 4040 par le shim", pt.target)).with_path(&pp));
                } else {
                    diags.push(Diagnostic::error("redirection-udp-multiple", format!("`{name}` : un seul port UDP peut être relayé depuis 4040 ; {} ne sera pas exposé publiquement (joignable dans le network group)", pt.target)).with_path(&pp));
                    continue;
                }
                Protocol::Udp
            }
            "tcp" => {
                if tcp.is_none() {
                    tcp = Some(TcpRelay { target_port: pt.target });
                    diags.push(Diagnostic::warning("redirection-tcp", format!("`{name}` : le port TCP {} publié devient une redirection TCP (port externe attribué par la plateforme), relayée depuis 4040 par le shim", pt.target)).with_path(&pp));
                } else {
                    diags.push(Diagnostic::error("redirection-tcp-multiple", format!("`{name}` : un seul port TCP peut être relayé depuis 4040 ; {} ne sera pas exposé publiquement (joignable dans le network group)", pt.target)).with_path(&pp));
                    continue;
                }
                Protocol::Tcp
            }
            other => {
                diags.push(Diagnostic::error("port-protocol-unsupported", format!("protocole `{other}` non supporté")).with_path(&pp));
                continue;
            }
        };
        out.push(Redirection { id: ResourceId(format!("redir/{name}/{}/{}", pt.protocol, pt.target)), app: ResourceId(format!("app/{name}")), namespace: namespace.clone(), protocol, target_port: pt.target });
    }
    (out, tcp, udp)
}

/// Corrélation par valeur : la valeur d'une variable du consommateur égale
/// un identifiant déclaré par un service substitué. Rôle levé par la clé
/// (`PASS`, `USER`, `DB`/`DATABASE`/`NAME`, `HOST`, `PORT`), sinon par
/// unicité de la correspondance ; ambigu → rien, avertissement dans le
/// journal du plan.
fn correlate(key: &str, value: &str, infos: &IndexMap<String, (addons::Recognized, addons::Declared)>) -> Option<(String, addons::Role)> {
    use addons::Role;
    let k = key.to_ascii_uppercase();
    let mut matches: Vec<(String, Role)> = Vec::new();
    for (name, (rec, d)) in infos {
        if d.password.as_deref() == Some(value) {
            matches.push((name.clone(), Role::Password));
        }
        if d.root_password.as_deref() == Some(value) && d.password.as_deref() != Some(value) {
            matches.push((name.clone(), Role::Password));
        }
        if d.user.as_deref() == Some(value) {
            matches.push((name.clone(), Role::User));
        }
        if d.database.as_deref() == Some(value) {
            matches.push((name.clone(), Role::Database));
        }
        if value == rec.port.to_string() && k.contains("PORT") {
            matches.push((name.clone(), Role::Port));
        }
    }
    if matches.is_empty() {
        return None;
    }
    let hinted = |role: Role| matches.iter().find(|(_, r)| *r == role).cloned();
    if k.contains("PASS") {
        return hinted(Role::Password);
    }
    if k.contains("USER") || k.contains("LOGIN") {
        return hinted(Role::User);
    }
    if k.contains("DATABASE") || k.contains("DBNAME") || k.contains("DB_NAME") || k.ends_with("_DB") || k.contains("_NAME") || k == "DB" {
        return hinted(Role::Database);
    }
    if k.contains("PORT") {
        return hinted(Role::Port);
    }
    let mut roles: Vec<Role> = matches.iter().map(|(_, r)| *r).collect();
    roles.dedup();
    if roles.len() == 1 { matches.into_iter().next() } else { None }
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
                diags.push(Diagnostic::warning("port-assumed-tcp", format!("`{name}` : le port {port} est supposé non HTTP ; le shim répondra au contrôle de santé sur 8080 dès qu'il écoute. `x-clever.protocol: http` s'il l'est")).with_path(p));
                PlatformPort::Health { ready_port: Some(*port) }
            } else {
                if candidates.len() > 1 && s.ports.is_empty() {
                    diags.push(Diagnostic::warning("multiple-ports", format!("`{name}` expose plusieurs ports ({:?}) : {port} est pris pour le port HTTP, les autres restent joignables dans le network group", candidates)).with_path(&p.child("expose")));
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
            diags.push(Diagnostic::warning("healthcheck-strict", format!("`{name}` : le healthcheck devient `CC_HEALTH_CHECK_PATH={after}` ; le déployeur exige alors une réponse 2xx sur ce chemin (le défaut `/` accepte jusqu'à 499)")).with_path(&p.child("healthcheck")));
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
                // Les addons ne sont pas ordonnés ici : créés avant les apps.
                let Some(dep_name) = dep.0.strip_prefix("app/") else { continue };
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
