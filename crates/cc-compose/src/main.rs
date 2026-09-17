//! CLI clever-compose : `clap` pour l'aide et les erreurs d'usage, options
//! globales alignées sur `docker compose` (`-f`, `-p`, `--project-directory`,
//! `--env-file`, `--profile`), sorties rendues par `ui`.

use std::path::{Path as FsPath, PathBuf};
use std::process::ExitCode;

use cc_compose::{compose, diagnostic};
use cc_compose_model::plan::*;
use clap::{Args, Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "clever-compose", version = build_version(), about = "Fait tourner un docker compose sur Clever Cloud : chaque service devient une application Docker, chaque réseau un network group, les bases reconnues des addons managés.", long_about = None, disable_help_subcommand = true, after_help = "Identifiants : CC_API_TOKEN (bearer), ou CLEVER_TOKEN + CLEVER_SECRET (OAuth 1.0). Organisation : --org ou CC_ORG.\nExemples :\n  clever-compose plan                 ce qui serait créé, avec les diagnostics\n  clever-compose up --yes             déploie (--yes confirme les addons, facturés)\n  clever-compose logs api --follow    suit les logs d'un service\n  clever-compose down --yes           supprime tout ce que le projet a créé")]
struct Cli {
    #[command(flatten)]
    project: ProjectArgs,
    #[command(subcommand)]
    command: Command,
}

/// Options de lecture du compose, comme `docker compose`.
#[derive(Args, Clone)]
#[command(next_help_heading = "Projet")]
struct ProjectArgs {
    /// Fichier compose (répétable ; sinon compose.yaml et son override)
    #[arg(short = 'f', long = "file", global = true, value_name = "FICHIER")]
    files: Vec<PathBuf>,
    /// Nom du projet (sinon `name:` du compose ou le répertoire)
    #[arg(short = 'p', long = "project-name", global = true, value_name = "NOM")]
    project_name: Option<String>,
    /// Répertoire du projet
    #[arg(long, global = true, value_name = "DIR")]
    project_directory: Option<PathBuf>,
    /// Fichier d'environnement pour l'interpolation (répétable ; sinon .env)
    #[arg(long, global = true, value_name = "FICHIER")]
    env_file: Vec<PathBuf>,
    /// Profil actif (répétable)
    #[arg(long, global = true, value_name = "PROFIL")]
    profile: Vec<String>,
}

/// Options du plan (ce que `plan`, `export`, `up` partagent).
#[derive(Args, Clone)]
#[command(next_help_heading = "Plan")]
struct PlanArgs {
    /// Accepte les volumes nommés des services non substituables comme stockage éphémère
    #[arg(long)]
    allow_ephemeral: bool,
    /// Fixture d'images (sortie de `images`) au lieu des registres
    #[arg(long, value_name = "FIXTURE.json", hide = true)]
    images: Option<PathBuf>,
}

/// Accès à la plateforme.
#[derive(Args, Clone)]
#[command(next_help_heading = "Plateforme")]
struct OrgArgs {
    /// Organisation (orga_xxx ou user_xxx) ; sinon CC_ORG
    #[arg(short = 'o', long, env = "CC_ORG", value_name = "ORG")]
    org: String,
}

#[derive(Subcommand)]
enum Command {
    /// Montre ce que `up` créerait : ressources, coût, diagnostics (aucun appel à l'API)
    Plan {
        #[command(flatten)]
        plan: PlanArgs,
        /// Le plan intermédiaire en JSON plutôt que le résumé
        #[arg(long, help_heading = "Sortie")]
        json: bool,
    },
    /// Le compose normalisé, comme `docker compose config`
    Config {
        #[arg(long, value_enum, default_value_t = ConfigFormat::Json)]
        format: ConfigFormat,
    },
    /// Crée ou met à jour les ressources sur Clever Cloud, puis déploie dans l'ordre des dépendances
    Up {
        #[command(flatten)]
        org: OrgArgs,
        #[command(flatten)]
        plan: PlanArgs,
        /// Montre ce qui serait fait, sans rien modifier
        #[arg(long, help_heading = "Exécution")]
        dry_run: bool,
        /// Confirme la création des addons (facturés)
        #[arg(short = 'y', long, help_heading = "Exécution")]
        yes: bool,
        /// N'attend pas la fin des déploiements
        #[arg(long, help_heading = "Exécution")]
        no_wait: bool,
        /// Conserve les fichiers du projet et le plan dans un bucket Cellar de l'organisation (relecture depuis la plateforme) ; `x-clever.assets: true` fait pareil
        #[arg(long, help_heading = "Exécution")]
        assets: bool,
    },
    /// Charge un dump local dans un addon (une fois par contenu)
    Seed {
        #[command(flatten)]
        org: OrgArgs,
        #[command(flatten)]
        plan: PlanArgs,
        /// Service substitué par l'addon
        #[arg(long, value_name = "SERVICE", help_heading = "Dump")]
        service: String,
        /// Dump : .sql/.sql.gz/.sh (PostgreSQL, MySQL), .js/.archive (MongoDB, DocumentDB), .redis/.resp, .ndjson (Elasticsearch)
        #[arg(long = "dump", value_name = "FICHIER", help_heading = "Dump")]
        file: PathBuf,
        /// Depuis ce poste (Docker s'il répond, sinon le client installé) plutôt qu'une Clever Task
        #[arg(long, help_heading = "Dump")]
        local: bool,
        /// Montre ce qui serait fait, sans rien modifier
        #[arg(long, help_heading = "Exécution")]
        dry_run: bool,
        /// Confirme la création de l'addon s'il n'existe pas (facturé)
        #[arg(short = 'y', long, help_heading = "Exécution")]
        yes: bool,
        /// N'attend pas la fin de la tâche
        #[arg(long, help_heading = "Exécution")]
        no_wait: bool,
    },
    /// Supprime ce que `up` a créé (par nom et tags, sans relire le compose)
    Down {
        #[command(flatten)]
        org: OrgArgs,
        /// Tous les projets clever-compose de l'organisation
        #[arg(long, help_heading = "Exécution")]
        all: bool,
        /// Montre ce qui serait supprimé
        #[arg(long, help_heading = "Exécution")]
        dry_run: bool,
        /// Supprime aussi les addons (données perdues)
        #[arg(short = 'y', long, help_heading = "Exécution")]
        yes: bool,
    },
    /// Arrête les applications du projet sans rien supprimer (comme `docker compose stop`)
    Stop {
        #[command(flatten)]
        org: OrgArgs,
    },
    /// Relance les applications arrêtées (comme `docker compose start`)
    Start {
        #[command(flatten)]
        org: OrgArgs,
    },
    /// État des applications, tâches, addons et network groups du projet
    Ps {
        #[command(flatten)]
        org: OrgArgs,
    },
    /// Logs des applications et addons du projet
    Logs {
        #[command(flatten)]
        org: OrgArgs,
        /// Services à afficher (sinon tous)
        #[arg(value_name = "SERVICE")]
        services: Vec<String>,
        /// Depuis une durée (10m, 2h, 1d) ou un horodatage RFC 3339
        #[arg(long, value_name = "DEPUIS", help_heading = "Sélection")]
        since: Option<String>,
        /// Nombre maximal de lignes par service
        #[arg(short = 'n', long = "tail", value_name = "N", help_heading = "Sélection")]
        limit: Option<usize>,
        /// Suit le flux (comme `docker compose logs --follow`)
        #[arg(long, help_heading = "Sélection")]
        follow: bool,
    },
    /// Écrit la traduction en Terraform et en script clever-tools
    Export {
        #[command(flatten)]
        plan: PlanArgs,
        /// Ce qui est écrit
        #[arg(value_enum, default_value_t = ExportWhat::All, help_heading = "Sortie")]
        what: ExportWhat,
        /// Répertoire de sortie
        #[arg(short = 'o', long, value_name = "DIR", help_heading = "Sortie")]
        output: PathBuf,
        /// Écrit les branches clever-compose/<service> dans le dépôt git du projet (pour deployment.repository)
        #[arg(long, help_heading = "Sortie")]
        branches: bool,
    },
    /// Organisations accessibles avec les identifiants (id et nom)
    Orgs,
    /// Connexion à Clever Cloud (danse OAuth 1.0 avec votre consumer CLEVER_CONSUMER_KEY/SECRET) ; jetons gardés dans ~/.config/clever-cloud/clever-compose.json
    Login {
        /// N'ouvre pas le navigateur (l'URL est affichée)
        #[arg(long)]
        no_open: bool,
        /// Port local du callback OAuth ; la base URL du consumer doit être http://127.0.0.1:<port>
        #[arg(long, default_value_t = 8080, value_name = "PORT")]
        callback_port: u16,
    },
    /// Oublie les jetons gardés par `login`
    Logout,
    /// Interface web locale : diagramme des ressources, plan, up, down, logs, édition du compose
    #[cfg(feature = "ui")]
    Ui {
        /// Organisation initiale (sinon choisie dans l'interface)
        #[arg(short = 'o', long, env = "CC_ORG", value_name = "ORG")]
        org: Option<String>,
        /// Adresse et port d'écoute (0.0.0.0:8080 dans une application Clever Cloud)
        #[arg(long, default_value = "127.0.0.1:8080", env = "CC_LISTEN", value_name = "ADRESSE:PORT")]
        listen: std::net::SocketAddr,
        /// N'ouvre pas le navigateur
        #[arg(long)]
        no_open: bool,
        /// Base publique du serveur si un proxy est devant (callback OAuth), ex. https://compose.exemple
        #[arg(long, value_name = "URL")]
        public_url: Option<String>,
        /// Clé exigée des clients de l'API (interface déployée à part) ; sinon aucune vérification
        #[arg(long, env = "CC_API_KEY", value_name = "CLÉ", hide_env_values = true)]
        api_key: Option<String>,
    },
    /// API seule, sans interface : pour une interface déployée à part (front Vite bâti avec VITE_API_URL)
    #[cfg(feature = "api")]
    Api {
        /// Organisation initiale (sinon choisie dans l'interface)
        #[arg(short = 'o', long, env = "CC_ORG", value_name = "ORG")]
        org: Option<String>,
        /// Adresse et port d'écoute (0.0.0.0:8080 dans une application Clever Cloud)
        #[arg(long, default_value = "127.0.0.1:8080", env = "CC_LISTEN", value_name = "ADRESSE:PORT")]
        listen: std::net::SocketAddr,
        /// Origine de l'interface autorisée en CORS (répétable), ex. https://ui.exemple
        #[arg(long, value_name = "ORIGINE")]
        cors: Vec<String>,
        /// Base publique de l'API si un proxy est devant (callback OAuth), ex. https://api.exemple
        #[arg(long, value_name = "URL")]
        public_url: Option<String>,
        /// Clé exigée des clients de l'API (interface déployée à part) ; sinon aucune vérification
        #[arg(long, env = "CC_API_KEY", value_name = "CLÉ", hide_env_values = true)]
        api_key: Option<String>,
    },
    /// Fixture des configurations d'images du projet, lues sur les registres (tests hors ligne)
    #[command(hide = true)]
    Images,
}

#[derive(Clone, Copy, ValueEnum)]
enum ConfigFormat {
    Json,
    Yaml,
}

#[derive(Clone, Copy, ValueEnum)]
enum ExportWhat {
    All,
    Terraform,
    Script,
}

/// `0.1.0 (a1b2c3d, 2026-09-16T19:12:00Z)` : version, commit et date du build.
fn build_version() -> &'static str {
    static V: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    V.get_or_init(|| {
        let stamp = env!("CC_BUILD_STAMP");
        let (commit, secs) = stamp.split_once(' ').unwrap_or((stamp, "0"));
        format!("{} ({commit}, {})", env!("CARGO_PKG_VERSION"), cc_compose::exec::inspect::rfc3339(secs.parse().unwrap_or(0)))
    })
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let opts = cli.project.to_options();
    match cli.command {
        Command::Plan { plan, json } => cmd_plan(opts, &plan, json),
        Command::Config { format } => cmd_config(opts, format),
        Command::Up { org, plan, dry_run, yes, no_wait, assets } => cmd_up(opts, &org.org, &plan, dry_run, yes, no_wait, assets, None),
        Command::Seed { org, plan, service, file, local, dry_run, yes, no_wait } => cmd_up(opts, &org.org, &plan, dry_run, yes, no_wait, false, Some((service, file, local))),
        Command::Down { org, all, dry_run, yes } => cmd_down(opts, &org.org, all, dry_run, yes),
        Command::Ps { org } => cmd_ps(opts, &org.org),
        Command::Stop { org } => cmd_stop_start(opts, &org.org, false),
        Command::Start { org } => cmd_stop_start(opts, &org.org, true),
        Command::Logs { org, services, since, limit, follow } => cmd_logs(opts, &org.org, cc_compose::exec::inspect::LogsOptions { services, since, follow, limit }),
        Command::Export { plan, what, output, branches } => cmd_export(opts, &plan, what, &output, branches),
        Command::Images => cmd_images(opts),
        #[cfg(feature = "ui")]
        Command::Ui { org, listen, no_open, public_url, api_key } => cmd_ui(opts, org.as_deref(), cc_compose::web::ServeOptions { listen, ui: true, origins: Vec::new(), public_url, api_key, initial_dir: None, open_browser: !no_open }),
        #[cfg(feature = "api")]
        Command::Api { org, listen, cors, public_url, api_key } => cmd_ui(opts, org.as_deref(), cc_compose::web::ServeOptions { listen, ui: false, origins: cors, public_url, api_key, initial_dir: None, open_browser: false }),
        Command::Orgs => cmd_orgs(),
        Command::Login { no_open, callback_port } => cmd_login(callback_port, !no_open),
        Command::Logout => match cc_compose::auth::remove() {
            Ok(true) => {
                cc_compose::ui::Ui::stderr().done(true, "jetons oubliés");
                ExitCode::SUCCESS
            }
            Ok(false) => {
                eprintln!("aucun jeton gardé ({})", cc_compose::auth::config_path().display());
                ExitCode::SUCCESS
            }
            Err(e) => fail(&e),
        },
    }
}

impl ProjectArgs {
    fn to_options(&self) -> compose::project::Options {
        let mut opts = compose::project::Options { project_dir: self.project_directory.clone().unwrap_or_else(|| PathBuf::from(".")), files: self.files.clone(), project_name: self.project_name.clone(), env_files: self.env_file.clone(), profiles: self.profile.clone() };
        // Comme docker compose : sans --project-directory, le répertoire du
        // premier -f est celui du projet.
        if self.project_directory.is_none() {
            if let Some(f) = opts.files.first() {
                opts.project_dir = compose::project::project_dir_of(f);
                let dir = opts.project_dir.clone();
                opts.files = opts.files.iter().map(|f| f.strip_prefix(&dir).map(PathBuf::from).unwrap_or(f.clone())).collect();
            }
        }
        opts
    }
}

fn fail(msg: &str) -> ExitCode {
    cc_compose::ui::Ui::stderr().done(false, msg);
    // Jetons refusés ou absents : proposer la connexion, tout de suite si
    // un humain est au clavier.
    if cc_compose::auth::looks_like_auth_failure(msg) {
        if std::io::IsTerminal::is_terminal(&std::io::stdin()) && std::io::IsTerminal::is_terminal(&std::io::stderr()) {
            eprint!("Se connecter maintenant avec `clever-compose login` ? [O/n] ");
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            if matches!(line.trim().to_ascii_lowercase().as_str(), "" | "o" | "oui" | "y" | "yes") {
                if cmd_login(8080, true) == ExitCode::SUCCESS {
                    eprintln!("relancez la commande.");
                }
                return ExitCode::from(1);
            }
        } else if !msg.contains("clever-compose login") {
            eprintln!("  → `clever-compose login`, ou CLEVER_TOKEN et CLEVER_SECRET (CC_API_TOKEN) dans l'environnement");
        }
    }
    ExitCode::from(1)
}

fn cmd_login(port: u16, open: bool) -> ExitCode {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    match rt.block_on(cc_compose::auth::login(port, open)) {
        Ok(s) => {
            cc_compose::ui::Ui::stderr().done(true, &format!("connecté{} ; jetons gardés dans {}", s.email.map(|e| format!(" en tant que {e}")).unwrap_or_default(), cc_compose::auth::config_path().display()));
            ExitCode::SUCCESS
        }
        Err(e) => {
            cc_compose::ui::Ui::stderr().done(false, &e);
            ExitCode::from(1)
        }
    }
}

fn resolver(images: &Option<PathBuf>) -> Result<Box<dyn cc_compose::plan::images::ImageResolver>, String> {
    Ok(match images {
        Some(path) => Box::new(cc_compose::plan::cli::fixture_from_file(path)?),
        None => Box::new(cc_compose::plan::registry::Registry::new()?),
    })
}

struct Computed {
    plan: Option<Plan>,
    project_dir: PathBuf,
    resolver: Box<dyn cc_compose::plan::images::ImageResolver>,
    diags: diagnostic::Diagnostics,
}

impl Computed {
    fn errors(&self) -> usize {
        self.diags.items.iter().filter(|d| d.severity == diagnostic::Severity::Error).count()
    }

    /// Diagnostics puis décompte ; `true` si le plan est utilisable.
    fn report(&self, ui: &mut cc_compose::ui::Ui) -> bool {
        for line in self.diags.render_all() {
            ui.diagnostic(&line);
        }
        let errors = self.errors();
        if !self.diags.is_empty() {
            ui.summary(errors, self.diags.items.len() - errors);
        }
        self.plan.is_some() && errors == 0
    }
}

/// Calcule le plan sans rien afficher.
fn compute_plan(opts: compose::project::Options, args: &PlanArgs) -> Result<Computed, String> {
    let resolver = resolver(&args.images)?;
    let mut diags = diagnostic::Diagnostics::default();
    let project_dir = std::fs::canonicalize(&opts.project_dir).unwrap_or_else(|_| opts.project_dir.clone());
    let popts = cc_compose::plan::cli::PlanOptions { project: opts, allow_ephemeral: args.allow_ephemeral, private_registries: None };
    let plan = cc_compose::plan::cli::run(&popts, resolver.as_ref(), &mut diags);
    Ok(Computed { plan, project_dir, resolver, diags })
}

fn cmd_plan(opts: compose::project::Options, args: &PlanArgs, json: bool) -> ExitCode {
    let mut ui = cc_compose::ui::Ui::stderr();
    let computed = match compute_plan(opts, args) {
        Ok(c) => c,
        Err(e) => return fail(&e),
    };
    if json {
        let ok = computed.report(&mut ui);
        if let (true, Some(plan)) = (ok, &computed.plan) {
            println!("{}", serde_json::to_string_pretty(plan).unwrap());
            return ExitCode::SUCCESS;
        }
        return ExitCode::from(1);
    }
    // Le résumé d'abord (ce qui serait créé), puis ce qu'il faut savoir ou
    // corriger.
    if let (Some(plan), 0) = (&computed.plan, computed.errors()) {
        ui.plan_summary(plan);
        if !computed.diags.is_empty() {
            ui.blank();
        }
    }
    if computed.report(&mut ui) {
        ExitCode::SUCCESS
    } else {
        ui.done(false, "plan refusé : corrigez les erreurs ci-dessus (`x-clever` pour les choix, voir README)");
        ExitCode::from(1)
    }
}

fn cmd_config(opts: compose::project::Options, format: ConfigFormat) -> ExitCode {
    let mut diags = diagnostic::Diagnostics::default();
    let project = compose::project::load(&opts, &mut diags);
    let mut ui = cc_compose::ui::Ui::stderr();
    for line in diags.render_all() {
        ui.diagnostic(&line);
    }
    let Some(project) = project.filter(|_| !diags.has_errors()) else { return ExitCode::from(1) };
    match format {
        ConfigFormat::Json => println!("{}", serde_json::to_string_pretty(&project.tree.to_json()).unwrap()),
        ConfigFormat::Yaml => return fail("--format yaml n'est pas encore disponible, utilisez --format json"),
    }
    ExitCode::SUCCESS
}

fn cmd_images(opts: compose::project::Options) -> ExitCode {
    let registry = match cc_compose::plan::registry::Registry::new() {
        Ok(r) => r,
        Err(e) => return fail(&e),
    };
    let mut diags = diagnostic::Diagnostics::default();
    let fixture = cc_compose::plan::cli::images_fixture(&opts, &registry, &mut diags);
    let mut ui = cc_compose::ui::Ui::stderr();
    for line in diags.render_all() {
        ui.diagnostic(&line);
    }
    match fixture {
        Some(f) if !diags.has_errors() => {
            println!("{}", serde_json::to_string_pretty(&f).unwrap());
            ExitCode::SUCCESS
        }
        _ => ExitCode::from(1),
    }
}

fn api(org: &str) -> Result<cc_compose::exec::platform::Api, String> {
    let (creds, _) = cc_compose::auth::credentials()?;
    Ok(cc_compose::exec::platform::Api::new(org.to_string(), creds))
}

#[allow(clippy::too_many_arguments)]
fn cmd_up(opts: compose::project::Options, org: &str, args: &PlanArgs, dry_run: bool, yes: bool, no_wait: bool, assets: bool, seed: Option<(String, PathBuf, bool)>) -> ExitCode {
    let api = match api(org) {
        Ok(a) => a,
        Err(e) => return fail(&e),
    };
    let ui = std::sync::Arc::new(std::sync::Mutex::new(cc_compose::ui::Ui::stderr()));
    let computed = match compute_plan(opts, args) {
        Ok(c) => c,
        Err(e) => return fail(&e),
    };
    if !computed.report(&mut ui.lock().unwrap()) {
        ui.lock().unwrap().done(false, "plan refusé : corrigez les erreurs ci-dessus");
        return ExitCode::from(1);
    }
    let Computed { plan: Some(plan), project_dir, resolver, diags } = computed else { return ExitCode::from(1) };
    if diags.is_empty() {
        // Rien à séparer des actions.
    }
    let plan = match &seed {
        None => plan,
        Some((service, file, local)) => match cc_compose::plan::initdb::seed_plan(&plan, service, file, &project_dir, resolver.as_ref(), *local) {
            Ok(p) => p,
            Err(e) => return fail(&format!("seed : {e}")),
        },
    };
    ui.lock().unwrap().blank();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let ui2 = ui.clone();
    let mut log: cc_compose::exec::reconcile::Log = Box::new(move |line: &str| ui2.lock().unwrap().log(line));
    let result = rt.block_on(cc_compose::exec::reconcile::up(&plan, &api, &cc_compose::exec::reconcile::Options { project_dir: &project_dir, dry_run, no_wait, confirm_addons: yes, assets }, &mut log));
    drop(log);
    let mut ui = ui.lock().unwrap();
    match result {
        Ok(state) => {
            if dry_run {
                ui.done(true, "rien n'a été modifié (--dry-run)");
                return ExitCode::SUCCESS;
            }
            // Les accès publics des services, pour finir.
            let mut urls: Vec<(String, String)> = Vec::new();
            for r in &plan.resources {
                let Resource::App(a) = r else { continue };
                if a.kind != AppKind::Service {
                    continue;
                }
                let Some(id) = state.app_ids.get(&a.service) else { continue };
                if let Ok(domains) = rt.block_on(cc_compose::exec::platform::Platform::list_domains(&api, id)) {
                    if let Some(d) = domains.first() {
                        urls.push((a.service.clone(), format!("https://{}/", d.trim_end_matches('/'))));
                    }
                }
            }
            ui.done(true, &format!("projet `{}` à jour", plan.project));
            for (service, url) in urls {
                ui.log(&format!("  {service} : {url}"));
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            ui.done(false, &e);
            ExitCode::from(1)
        }
    }
}

/// Nom du projet pour les commandes sans plan : `-p`, `COMPOSE_PROJECT_NAME`,
/// le compose s'il se charge, sinon le répertoire.
fn project_name(opts: &compose::project::Options) -> String {
    if let Some(n) = opts.project_name.clone().or_else(|| std::env::var("COMPOSE_PROJECT_NAME").ok().filter(|n| !n.is_empty())) {
        return n;
    }
    let mut diags = diagnostic::Diagnostics::default();
    match compose::project::load(opts, &mut diags) {
        Some(p) => p.name,
        None => {
            let dir = std::fs::canonicalize(&opts.project_dir).unwrap_or_else(|_| opts.project_dir.clone());
            compose::project::normalize_project_name(dir.file_name().and_then(|s| s.to_str()).unwrap_or("default"))
        }
    }
}

fn run_stateless(f: impl std::future::Future<Output = Result<(), String>>) -> ExitCode {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    match rt.block_on(f) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => fail(&e),
    }
}

fn ui_log(stdout: bool) -> cc_compose::exec::reconcile::Log {
    if stdout {
        return Box::new(|line: &str| println!("{line}"));
    }
    let ui = std::sync::Mutex::new(cc_compose::ui::Ui::stderr());
    Box::new(move |line: &str| ui.lock().unwrap().log(line))
}

fn cmd_down(opts: compose::project::Options, org: &str, all: bool, dry_run: bool, yes: bool) -> ExitCode {
    let api = match api(org) {
        Ok(a) => a,
        Err(e) => return fail(&e),
    };
    let mut log = ui_log(false);
    if !all {
        let project = project_name(&opts);
        return run_stateless(async {
            cc_compose::exec::reconcile::down(&project, &api, dry_run, yes, &mut log).await?;
            cc_compose::ui::Ui::stderr().done(true, if dry_run { "rien n'a été modifié (--dry-run)" } else { "projet supprimé" });
            Ok(())
        });
    }
    run_stateless(async {
        let projects = cc_compose::exec::inspect::projects(&api).await?;
        if projects.is_empty() {
            log("aucun projet clever-compose dans cette organisation");
            return Ok(());
        }
        log(&format!("projets : {}", projects.join(", ")));
        for p in &projects {
            log(&format!("== {p}"));
            cc_compose::exec::reconcile::down(p, &api, dry_run, yes, &mut log).await?;
        }
        cc_compose::ui::Ui::stderr().done(true, if dry_run { "rien n'a été modifié (--dry-run)" } else { "organisation nettoyée" });
        Ok(())
    })
}

fn cmd_stop_start(opts: compose::project::Options, org: &str, start: bool) -> ExitCode {
    let api = match api(org) {
        Ok(a) => a,
        Err(e) => return fail(&e),
    };
    let project = project_name(&opts);
    let mut log = ui_log(false);
    run_stateless(async {
        cc_compose::exec::reconcile::stop_start(&project, &api, start, &mut log).await?;
        cc_compose::ui::Ui::stderr().done(true, if start { "projet démarré" } else { "projet arrêté (down pour supprimer)" });
        Ok(())
    })
}

fn cmd_ps(opts: compose::project::Options, org: &str) -> ExitCode {
    let api = match api(org) {
        Ok(a) => a,
        Err(e) => return fail(&e),
    };
    let project = project_name(&opts);
    let mut log = ui_log(true);
    run_stateless(cc_compose::exec::inspect::ps(&project, &api, &mut log))
}

fn cmd_logs(opts: compose::project::Options, org: &str, lopts: cc_compose::exec::inspect::LogsOptions) -> ExitCode {
    let api = match api(org) {
        Ok(a) => a,
        Err(e) => return fail(&e),
    };
    let project = project_name(&opts);
    let mut log = ui_log(true);
    run_stateless(cc_compose::exec::inspect::logs(&project, &api, &lopts, &mut log))
}

#[cfg(feature = "api")]
fn cmd_ui(opts: compose::project::Options, org: Option<&str>, mut serve: cc_compose::web::ServeOptions) -> ExitCode {
    // Le consumer est obligatoire (danse des navigateurs, signature des
    // jetons sans consumer) ; les identifiants du serveur sont optionnels.
    let session = match (cc_compose::auth::credentials(), org) {
        (Ok((creds, _)), Some(org)) => Some(cc_compose::web::Session { api: cc_compose::exec::platform::Api::new(org.to_string(), creds), org: org.to_string() }),
        (Ok((creds, _)), None) => Some(cc_compose::web::Session { api: cc_compose::exec::platform::Api::new(String::new(), creds), org: String::new() }),
        (Err(_), _) => None,
    };
    let oauth = match cc_compose::web::OauthConsumer::from_env() {
        Ok(c) => c,
        Err(e) => return fail(&e),
    };
    serve.initial_dir = std::fs::canonicalize(&opts.project_dir).ok();
    run_stateless(cc_compose::web::serve(serve, session, oauth))
}

fn cmd_orgs() -> ExitCode {
    let creds = match cc_compose::auth::credentials() {
        Ok((c, _)) => c,
        Err(e) => return fail(&e),
    };
    let api = cc_compose::exec::platform::Api::new(String::new(), creds);
    run_stateless(async {
        let orgs = api.organisations().await?;
        let w = orgs.iter().map(|o| o.id.len()).max().unwrap_or(0);
        for o in orgs {
            println!("{:<w$}  {}", o.id, o.name);
        }
        Ok(())
    })
}

fn cmd_export(opts: compose::project::Options, args: &PlanArgs, what: ExportWhat, out: &FsPath, branches: bool) -> ExitCode {
    let what = cc_compose::emit::export::What { terraform: !matches!(what, ExportWhat::Script), script: !matches!(what, ExportWhat::Terraform), repos: true, branches };
    let mut ui = cc_compose::ui::Ui::stderr();
    let computed = match compute_plan(opts, args) {
        Ok(c) => c,
        Err(e) => return fail(&e),
    };
    if !computed.report(&mut ui) {
        ui.done(false, "plan refusé : corrigez les erreurs ci-dessus");
        return ExitCode::from(1);
    }
    let Computed { plan: Some(plan), project_dir, .. } = computed else { return ExitCode::from(1) };
    let mut diags = diagnostic::Diagnostics::default();
    let result = cc_compose::emit::export::write(&plan, &project_dir, out, &what, &mut diags);
    if let Err(d) = &result {
        diags.push(d.clone());
    }
    for line in diags.render_all() {
        ui.diagnostic(&line);
    }
    if diags.has_errors() {
        ui.done(false, "export incomplet : corrigez les erreurs ci-dessus");
        return ExitCode::from(1);
    }
    ui.done(true, &format!("export écrit dans {}", out.display()));
    ExitCode::SUCCESS
}
