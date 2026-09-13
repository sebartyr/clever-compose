//! CLI clever-compose.

use cc_compose::{compose, diagnostic};

use std::path::PathBuf;
use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!("usage : clever-compose config [--project-directory DIR] [-f FILE]... [-p NAME] [--env-file F] [--profile P] [--format json]");
    eprintln!("        clever-compose plan   [mêmes options] [--images FIXTURE.json] [--allow-ephemeral]");
    eprintln!("        clever-compose images --project-directory DIR");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("config") => config(&args[1..], false),
        Some("plan") => config(&args[1..], true),
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
