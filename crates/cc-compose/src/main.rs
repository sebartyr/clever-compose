//! CLI clever-compose.

use cc_compose::{compose, diagnostic};

use std::path::PathBuf;
use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!("usage : clever-compose config [--project-directory DIR] [-f FILE]... [-p NAME] [--env-file F] [--profile P] [--format json]");
    eprintln!("        clever-compose plan   [mêmes options] [--images FIXTURE.json] [--allow-ephemeral]");
    eprintln!("        clever-compose images --project-directory DIR");
    eprintln!("        clever-compose export [terraform|script|all] -o DIR [--branches] [options de plan]");
    eprintln!("          --branches écrit les branches clever-compose/<service> dans le dépôt git du projet");
    eprintln!("        clever-compose up --org ORG [--dry-run] [--no-wait] [--yes] [options de plan]");
    eprintln!("        clever-compose seed --service SVC --file DUMP.sql [--local] [mêmes options que up]");
    eprintln!("          --local charge le dump depuis ce poste (Docker ou client local) au lieu d'une tâche");
    eprintln!("        clever-compose down --org ORG [-p PROJET | --all] [--dry-run] [--yes]  (sans plan ; --yes supprime aussi les addons ; --all : tous les projets de l'organisation)");
    eprintln!("        clever-compose ps   --org ORG [-p PROJET]                               (état des apps, addons, réseaux)");
    eprintln!("        clever-compose logs --org ORG [-p PROJET] [SERVICE...] [--since 1h] [-n N] [-f]");
    eprintln!("          identifiants : CC_API_TOKEN, ou CLEVER_TOKEN + CLEVER_SECRET (OAuth 1.0)");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("config") => config(&args[1..], false),
        Some("plan") => config(&args[1..], true),
        Some("export") => export(&args[1..]),
        Some("up") => up(&args[1..], None),
        Some("down") => down(&args[1..]),
        Some("ps") => inspect(&args[1..], Inspect::Ps),
        Some("logs") => inspect(&args[1..], Inspect::Logs),
        Some("seed") => {
            // clever-compose seed --service SVC --file DUMP [options de up]
            let mut rest: Vec<String> = Vec::new();
            let (mut service, mut file) = (None, None);
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--service" => {
                        i += 1;
                        service = args.get(i).cloned();
                    }
                    "--file" => {
                        i += 1;
                        file = args.get(i).map(PathBuf::from);
                    }
                    other => rest.push(other.to_string()),
                }
                i += 1;
            }
            match (service, file) {
                (Some(s), Some(f)) => up(&rest, Some((s, f))),
                _ => {
                    eprintln!("clever-compose seed : --service SVC et --file DUMP sont requis");
                    ExitCode::from(2)
                }
            }
        }
        Some("images") => images(&args[1..]),
        _ => usage(),
    }
}

/// `clever-compose images --project-directory DIR` : fixture d'images du
/// projet, lue sur les registres, au format de `--images`.
fn images(args: &[String]) -> ExitCode {
    let mut opts = compose::project::Options { project_dir: PathBuf::from("."), files: Vec::new(), project_name: None, env_files: Vec::new(), profiles: Vec::new() };
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--project-directory" => {
                i += 1;
                opts.project_dir = PathBuf::from(args.get(i).cloned().unwrap_or_default());
            }
            _ => return usage(),
        }
        i += 1;
    }
    let registry = match cc_compose::plan::registry::Registry::new() {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(1);
        }
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

fn up(args: &[String], seed: Option<(String, PathBuf)>) -> ExitCode {
    let mut rest: Vec<String> = Vec::new();
    let mut org: Option<String> = std::env::var("CC_ORG").ok();
    let mut dry_run = false;
    let mut no_wait = false;
    let mut confirm_addons = false;
    let mut seed_local = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--org" | "-o" => {
                i += 1;
                org = args.get(i).cloned();
            }
            "--dry-run" => dry_run = true,
            "--local" => seed_local = true,
            "--no-wait" => no_wait = true,
            "--yes" | "-y" => confirm_addons = true,
            other => rest.push(other.to_string()),
        }
        i += 1;
    }
    let Some(org) = org else {
        eprintln!("clever-compose up : --org ORG (ou CC_ORG) est requis");
        return ExitCode::from(2);
    };
    let (opts, images, allow_ephemeral) = match parse_plan_args(&rest) {
        Some(x) => x,
        None => return usage(),
    };
    let creds = match cc_compose::exec::platform::credentials_from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let resolver: Box<dyn cc_compose::plan::images::ImageResolver> = match images {
        Some(path) => match cc_compose::plan::cli::fixture_from_file(&path) {
            Ok(f) => Box::new(f),
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(1);
            }
        },
        None => match cc_compose::plan::registry::Registry::new() {
            Ok(r) => Box::new(r),
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(1);
            }
        },
    };
    let mut diags = diagnostic::Diagnostics::default();
    let project_dir = std::fs::canonicalize(&opts.project_dir).unwrap_or_else(|_| opts.project_dir.clone());
    let popts = cc_compose::plan::cli::PlanOptions { project: opts, allow_ephemeral, private_registries: None };
    let plan = cc_compose::plan::cli::run(&popts, resolver.as_ref(), &mut diags);
    let ui = std::sync::Arc::new(std::sync::Mutex::new(cc_compose::ui::Ui::stderr()));
    for line in diags.render_all() {
        ui.lock().unwrap().diagnostic(&line);
    }
    let Some(plan) = plan else {
        ui.lock().unwrap().done(false, "plan impossible : corrigez les erreurs ci-dessus");
        return ExitCode::from(1);
    };
    if diags.has_errors() {
        ui.lock().unwrap().done(false, "plan refusé : corrigez les erreurs ci-dessus");
        return ExitCode::from(1);
    }
    if !diags.is_empty() {
        ui.lock().unwrap().blank();
    }
    let plan = match &seed {
        None => plan,
        Some((service, file)) => match cc_compose::plan::initdb::seed_plan(&plan, service, file, &project_dir, resolver.as_ref(), seed_local) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("seed : {e}");
                return ExitCode::from(1);
            }
        },
    };
    let api = cc_compose::exec::platform::Api::new(org, creds);
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let ui2 = ui.clone();
    let mut log: cc_compose::exec::reconcile::Log = Box::new(move |line: &str| ui2.lock().unwrap().log(line));
    let result = rt.block_on(cc_compose::exec::reconcile::up(&plan, &api, &cc_compose::exec::reconcile::Options { project_dir: &project_dir, dry_run, no_wait, confirm_addons }, &mut log));
    drop(log);
    match result {
        Ok(_) => {
            ui.lock().unwrap().done(true, if dry_run { "rien n'a été modifié (--dry-run)" } else { "projet à jour" });
            ExitCode::SUCCESS
        }
        Err(e) => {
            ui.lock().unwrap().done(false, &e);
            ExitCode::from(1)
        }
    }
}

/// Les commandes sans plan (`down`, `ps`, `logs`) : le nom du projet et
/// l'accès à l'API suffisent, l'état est relu depuis la plateforme. Le nom
/// vient de `-p`, de `COMPOSE_PROJECT_NAME`, du compose s'il se charge
/// encore, sinon du répertoire du projet.
struct Stateless {
    project: String,
    api: cc_compose::exec::platform::Api,
    /// Arguments non consommés (propres à la commande).
    rest: Vec<String>,
}

fn stateless(cmd: &str, args: &[String], consume: &mut dyn FnMut(&str, &mut usize, &[String]) -> bool) -> Result<Stateless, ExitCode> {
    let mut rest: Vec<String> = Vec::new();
    let mut org: Option<String> = std::env::var("CC_ORG").ok();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--org" | "-o" => {
                i += 1;
                org = args.get(i).cloned();
            }
            // Options de plan à valeur : la valeur n'est pas un positionnel.
            o @ ("-p" | "--project-name" | "--project-directory" | "-f" | "--file" | "--env-file" | "--profile" | "--images") => {
                rest.push(o.to_string());
                i += 1;
                rest.extend(args.get(i).cloned());
            }
            other => {
                if !consume(other, &mut i, args) {
                    rest.push(other.to_string());
                }
            }
        }
        i += 1;
    }
    let Some(org) = org else {
        eprintln!("clever-compose {cmd} : --org ORG (ou CC_ORG) est requis");
        return Err(ExitCode::from(2));
    };
    let Some((opts, _, _)) = parse_plan_args(&rest) else { return Err(usage()) };
    let creds = cc_compose::exec::platform::credentials_from_env().map_err(|e| {
        eprintln!("{e}");
        ExitCode::from(2)
    })?;
    let project = match opts.project_name.clone().or_else(|| std::env::var("COMPOSE_PROJECT_NAME").ok().filter(|n| !n.is_empty())) {
        Some(n) => n,
        None => {
            let mut diags = diagnostic::Diagnostics::default();
            match cc_compose::compose::project::load(&opts, &mut diags) {
                Some(p) => p.name,
                None => {
                    let dir = std::fs::canonicalize(&opts.project_dir).unwrap_or_else(|_| opts.project_dir.clone());
                    cc_compose::compose::project::normalize_project_name(dir.file_name().and_then(|s| s.to_str()).unwrap_or("default"))
                }
            }
        }
    };
    Ok(Stateless { project, api: cc_compose::exec::platform::Api::new(org, creds), rest: Vec::new() })
}

fn run_stateless(f: impl std::future::Future<Output = Result<(), String>>) -> ExitCode {
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    match rt.block_on(f) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            cc_compose::ui::Ui::stderr().done(false, &e);
            ExitCode::from(1)
        }
    }
}

/// Journal d'actions rendu par l'interface terminal.
fn ui_log() -> cc_compose::exec::reconcile::Log {
    let ui = std::sync::Mutex::new(cc_compose::ui::Ui::stderr());
    Box::new(move |line: &str| ui.lock().unwrap().log(line))
}

fn down(args: &[String]) -> ExitCode {
    let mut dry_run = false;
    let mut confirm_addons = false;
    let mut all = false;
    let st = match stateless("down", args, &mut |a, _, _| match a {
        "--dry-run" => {
            dry_run = true;
            true
        }
        "--yes" | "-y" => {
            confirm_addons = true;
            true
        }
        "--all" => {
            all = true;
            true
        }
        _ => false,
    }) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let _ = &st.rest;
    let mut log = ui_log();
    if !all {
        return run_stateless(cc_compose::exec::reconcile::down(&st.project, &st.api, dry_run, confirm_addons, &mut log));
    }
    // `--all` : tous les projets que clever-compose a créés dans
    // l'organisation, retrouvés par leur tag ; les addons seulement avec --yes.
    run_stateless(async {
        let projects = cc_compose::exec::inspect::projects(&st.api).await?;
        if projects.is_empty() {
            log("aucun projet clever-compose dans cette organisation");
            return Ok(());
        }
        log(&format!("projets : {}", projects.join(", ")));
        for p in &projects {
            log(&format!("== {p}"));
            cc_compose::exec::reconcile::down(p, &st.api, dry_run, confirm_addons, &mut log).await?;
        }
        Ok(())
    })
}

enum Inspect {
    Ps,
    Logs,
}

fn inspect(args: &[String], what: Inspect) -> ExitCode {
    let mut lopts = cc_compose::exec::inspect::LogsOptions::default();
    let st = match stateless(if matches!(what, Inspect::Ps) { "ps" } else { "logs" }, args, &mut |a, i, args| match a {
        "-f" | "--follow" if matches!(what, Inspect::Logs) => {
            lopts.follow = true;
            true
        }
        "--since" => {
            *i += 1;
            lopts.since = args.get(*i).cloned();
            true
        }
        "-n" | "--tail" => {
            *i += 1;
            lopts.limit = args.get(*i).and_then(|v| v.parse().ok());
            true
        }
        a if !a.starts_with('-') && matches!(what, Inspect::Logs) => {
            lopts.services.push(a.to_string());
            true
        }
        _ => false,
    }) {
        Ok(s) => s,
        Err(code) => return code,
    };
    let mut log: cc_compose::exec::reconcile::Log = Box::new(|line: &str| println!("{line}"));
    match what {
        Inspect::Ps => run_stateless(cc_compose::exec::inspect::ps(&st.project, &st.api, &mut log)),
        Inspect::Logs => run_stateless(cc_compose::exec::inspect::logs(&st.project, &st.api, &lopts, &mut log)),
    }
}

fn export(args: &[String]) -> ExitCode {
    let mut what = cc_compose::emit::export::What { terraform: true, script: true, repos: true, branches: false };
    let mut rest: Vec<String> = Vec::new();
    let mut out: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "terraform" if i == 0 => what = cc_compose::emit::export::What { terraform: true, script: false, repos: true, branches: false },
            "script" if i == 0 => what = cc_compose::emit::export::What { terraform: false, script: true, repos: true, branches: false },
            "all" if i == 0 => {}
            "--branches" => what.branches = true,
            "-o" | "--output" => {
                i += 1;
                out = args.get(i).map(PathBuf::from);
            }
            other => rest.push(other.to_string()),
        }
        i += 1;
    }
    let Some(out) = out else {
        eprintln!("clever-compose export : -o DIR est requis");
        return ExitCode::from(2);
    };
    let (opts, images, allow_ephemeral) = match parse_plan_args(&rest) {
        Some(x) => x,
        None => return usage(),
    };
    let resolver: Box<dyn cc_compose::plan::images::ImageResolver> = match images {
        Some(path) => match cc_compose::plan::cli::fixture_from_file(&path) {
            Ok(f) => Box::new(f),
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(1);
            }
        },
        None => match cc_compose::plan::registry::Registry::new() {
            Ok(r) => Box::new(r),
            Err(e) => {
                eprintln!("{e}");
                return ExitCode::from(1);
            }
        },
    };
    let mut diags = diagnostic::Diagnostics::default();
    let project_dir = std::fs::canonicalize(&opts.project_dir).unwrap_or_else(|_| opts.project_dir.clone());
    let popts = cc_compose::plan::cli::PlanOptions { project: opts, allow_ephemeral, private_registries: None };
    let plan = cc_compose::plan::cli::run(&popts, resolver.as_ref(), &mut diags);
    let result = plan.as_ref().map(|p| cc_compose::emit::export::write(p, &project_dir, &out, &what, &mut diags));
    if let Some(Err(d)) = &result {
        diags.push(d.clone());
    }
    let mut ui = cc_compose::ui::Ui::stderr();
    for line in diags.render_all() {
        ui.diagnostic(&line);
    }
    if plan.is_none() || diags.has_errors() {
        return ExitCode::from(1);
    }
    eprintln!("export écrit dans {}", out.display());
    ExitCode::SUCCESS
}

/// Options communes de `plan` et `export`.
fn parse_plan_args(args: &[String]) -> Option<(compose::project::Options, Option<PathBuf>, bool)> {
    let mut opts = compose::project::Options { project_dir: PathBuf::from("."), files: Vec::new(), project_name: None, env_files: Vec::new(), profiles: Vec::new() };
    let mut images: Option<PathBuf> = None;
    let mut allow_ephemeral = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--images" => {
                i += 1;
                images = args.get(i).map(PathBuf::from);
            }
            "--allow-ephemeral" => allow_ephemeral = true,
            "--project-directory" => {
                i += 1;
                opts.project_dir = PathBuf::from(args.get(i).cloned().unwrap_or_default());
            }
            "-f" | "--file" => {
                i += 1;
                opts.files.push(PathBuf::from(args.get(i).cloned().unwrap_or_default()));
            }
            "-p" | "--project-name" => {
                i += 1;
                opts.project_name = args.get(i).cloned();
            }
            "--profile" => {
                i += 1;
                opts.profiles.push(args.get(i).cloned().unwrap_or_default());
            }
            "--env-file" => {
                i += 1;
                opts.env_files.push(PathBuf::from(args.get(i).cloned().unwrap_or_default()));
            }
            _ => return None,
        }
        i += 1;
    }
    Some((opts, images, allow_ephemeral))
}

fn config(args: &[String], plan: bool) -> ExitCode {
    let mut opts = compose::project::Options { project_dir: PathBuf::from("."), files: Vec::new(), project_name: None, env_files: Vec::new(), profiles: Vec::new() };
    let mut format = if plan { "json".to_string() } else { "yaml".to_string() };
    let mut images: Option<PathBuf> = None;
    let mut allow_ephemeral = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--images" => {
                i += 1;
                images = args.get(i).map(PathBuf::from);
            }
            "--allow-ephemeral" => allow_ephemeral = true,
            "--project-directory" => {
                i += 1;
                opts.project_dir = PathBuf::from(args.get(i).cloned().unwrap_or_default());
            }
            "-f" | "--file" => {
                i += 1;
                opts.files.push(PathBuf::from(args.get(i).cloned().unwrap_or_default()));
            }
            "-p" | "--project-name" => {
                i += 1;
                opts.project_name = args.get(i).cloned();
            }
            "--profile" => {
                i += 1;
                opts.profiles.push(args.get(i).cloned().unwrap_or_default());
            }
            "--env-file" => {
                i += 1;
                opts.env_files.push(PathBuf::from(args.get(i).cloned().unwrap_or_default()));
            }
            "--format" => {
                i += 1;
                format = args.get(i).cloned().unwrap_or_default();
            }
            _ => return usage(),
        }
        i += 1;
    }
    if opts.project_dir == std::path::Path::new(".") {
        if let Some(f) = opts.files.first() {
            opts.project_dir = compose::project::project_dir_of(f);
            let dir = opts.project_dir.clone();
            opts.files = opts.files.iter().map(|f| f.strip_prefix(&dir).map(PathBuf::from).unwrap_or(f.clone())).collect();
        }
    }

    let mut diags = diagnostic::Diagnostics::default();
    if plan {
        // `--images FIXTURE.json` remplace le registre (tests hors ligne).
        let resolver: Box<dyn cc_compose::plan::images::ImageResolver> = match images {
            Some(path) => match cc_compose::plan::cli::fixture_from_file(&path) {
                Ok(f) => Box::new(f),
                Err(e) => {
                    eprintln!("{e}");
                    return ExitCode::from(1);
                }
            },
            None => match cc_compose::plan::registry::Registry::new() {
                Ok(r) => Box::new(r),
                Err(e) => {
                    eprintln!("{e}");
                    return ExitCode::from(1);
                }
            },
        };
        let popts = cc_compose::plan::cli::PlanOptions { project: opts, allow_ephemeral, private_registries: None };
        let result = cc_compose::plan::cli::run(&popts, resolver.as_ref(), &mut diags);
        let mut ui = cc_compose::ui::Ui::stderr();
        for line in diags.render_all() {
            ui.diagnostic(&line);
        }
        let Some(result) = result else {
            return ExitCode::from(1);
        };
        println!("{}", serde_json::to_string_pretty(&result).unwrap());
        return ExitCode::SUCCESS;
    }
    let project = compose::project::load(&opts, &mut diags);
    let mut ui = cc_compose::ui::Ui::stderr();
    for line in diags.render_all() {
        ui.diagnostic(&line);
    }
    let Some(project) = project else {
        return ExitCode::from(1);
    };
    if diags.has_errors() {
        return ExitCode::from(1);
    }
    match format.as_str() {
        "json" => println!("{}", serde_json::to_string_pretty(&project.tree.to_json()).unwrap()),
        _ => {
            eprintln!("format `{format}` non supporté pour l'instant, utilisez --format json");
            return ExitCode::from(2);
        }
    }
    ExitCode::SUCCESS
}
