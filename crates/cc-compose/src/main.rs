//! CLI clever-compose.

use cc_compose::{compose, diagnostic};

use std::path::PathBuf;
use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!("usage : clever-compose config [--project-directory DIR] [-f FILE]... [-p NAME] [--env-file F] [--profile P] [--format json]");
    eprintln!("        clever-compose plan   [mêmes options] [--images FIXTURE.json] [--allow-ephemeral]");
    eprintln!("        clever-compose images --project-directory DIR");
    eprintln!("        clever-compose export [terraform|script|all] -o DIR [options de plan]");
    eprintln!("        clever-compose up --org ORG [--dry-run] [--no-wait] [--yes] [options de plan]");
    eprintln!("          identifiants : CC_API_TOKEN, ou CLEVER_TOKEN + CLEVER_SECRET (OAuth 1.0)");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("config") => config(&args[1..], false),
        Some("plan") => config(&args[1..], true),
        Some("export") => export(&args[1..]),
        Some("up") => up(&args[1..]),
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
    for line in diags.render_all() {
        eprintln!("{line}");
    }
    match fixture {
        Some(f) if !diags.has_errors() => {
            println!("{}", serde_json::to_string_pretty(&f).unwrap());
            ExitCode::SUCCESS
        }
        _ => ExitCode::from(1),
    }
}

fn up(args: &[String]) -> ExitCode {
    let mut rest: Vec<String> = Vec::new();
    let mut org: Option<String> = std::env::var("CC_ORG").ok();
    let mut dry_run = false;
    let mut no_wait = false;
    let mut confirm_addons = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--org" | "-o" => {
                i += 1;
                org = args.get(i).cloned();
            }
            "--dry-run" => dry_run = true,
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
    let popts = cc_compose::plan::cli::PlanOptions { project: opts, allow_ephemeral };
    let plan = cc_compose::plan::cli::run(&popts, resolver.as_ref(), &mut diags);
    for line in diags.render_all() {
        eprintln!("{line}");
    }
    let Some(plan) = plan else {
        return ExitCode::from(1);
    };
    if diags.has_errors() {
        return ExitCode::from(1);
    }
    let api = cc_compose::exec::platform::Api::new(org, creds);
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().unwrap();
    let mut log: cc_compose::exec::reconcile::Log = Box::new(|line: &str| eprintln!("{line}"));
    let result = rt.block_on(cc_compose::exec::reconcile::up(&plan, &api, &cc_compose::exec::reconcile::Options { project_dir: &project_dir, dry_run, no_wait, confirm_addons }, &mut log));
    match result {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("erreur : {e}");
            ExitCode::from(1)
        }
    }
}

fn export(args: &[String]) -> ExitCode {
    let mut what = cc_compose::emit::export::What { terraform: true, script: true, repos: true };
    let mut rest: Vec<String> = Vec::new();
    let mut out: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "terraform" if i == 0 => what = cc_compose::emit::export::What { terraform: true, script: false, repos: true },
            "script" if i == 0 => what = cc_compose::emit::export::What { terraform: false, script: true, repos: true },
            "all" if i == 0 => {}
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
    let popts = cc_compose::plan::cli::PlanOptions { project: opts, allow_ephemeral };
    let plan = cc_compose::plan::cli::run(&popts, resolver.as_ref(), &mut diags);
    let result = plan.as_ref().map(|p| cc_compose::emit::export::write(p, &project_dir, &out, &what, &mut diags));
    if let Some(Err(d)) = &result {
        diags.push(d.clone());
    }
    for line in diags.render_all() {
        eprintln!("{line}");
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
    if opts.project_dir == PathBuf::from(".") {
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
        let popts = cc_compose::plan::cli::PlanOptions { project: opts, allow_ephemeral };
        let result = cc_compose::plan::cli::run(&popts, resolver.as_ref(), &mut diags);
        for line in diags.render_all() {
            eprintln!("{line}");
        }
        let Some(result) = result else {
            return ExitCode::from(1);
        };
        println!("{}", serde_json::to_string_pretty(&result).unwrap());
        return ExitCode::SUCCESS;
    }
    let project = compose::project::load(&opts, &mut diags);
    for line in diags.render_all() {
        eprintln!("{line}");
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
