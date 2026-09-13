//! CLI clever-compose.

use cc_compose::{compose, diagnostic};

use std::path::PathBuf;
use std::process::ExitCode;

fn usage() -> ExitCode {
    eprintln!("usage : clever-compose config [--project-directory DIR] [-f FILE]... [-p NAME] [--format json]");
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("config") => config(&args[1..]),
        _ => usage(),
    }
}

fn config(args: &[String]) -> ExitCode {
    let mut opts = compose::project::Options { project_dir: PathBuf::from("."), files: Vec::new(), project_name: None, env_files: Vec::new() };
    let mut format = "yaml".to_string();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
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
    let project = compose::project::load(&opts, &mut diags);
    let files = project.as_ref().map(|p| p.files.clone()).unwrap_or_default();
    for d in &diags.items {
        eprintln!("{}", files.render(d));
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
