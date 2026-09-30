//! Projet déposé (`Options::confine`) : aucun fichier lu hors de son
//! répertoire, quelle que soit la clé Compose qui le désigne (audit #12,
//! S2). Sans confinement, le comportement Compose (chemins absolus, `~`)
//! reste celui de la CLI.

use std::path::{Path, PathBuf};

use cc_compose::compose::project::{load, Options};
use cc_compose::diagnostic::Diagnostics;

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let p = std::env::temp_dir().join(format!("cc-compose-confine-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(std::fs::canonicalize(p).unwrap())
    }
    fn write(&self, rel: &str, content: &str) {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Charge le projet ; rend les codes des erreurs.
fn codes(dir: &Path, confine: bool) -> Vec<String> {
    let opts = Options { project_dir: dir.to_path_buf(), files: Vec::new(), project_name: None, env_files: Vec::new(), profiles: Vec::new(), confine };
    let mut diags = Diagnostics::default();
    let _ = load(&opts, &mut diags);
    diags.items.iter().filter(|d| d.severity == cc_compose::diagnostic::Severity::Error).map(|d| d.code.to_string()).collect()
}

/// Un fichier hors du projet, lisible, que chaque cas essaie d'atteindre.
fn outside() -> Dir {
    let d = Dir::new("outside");
    d.write("secret.env", "LEAK=1\n");
    d.write("base.yaml", "services:\n  base:\n    image: alpine\n");
    d
}

#[test]
fn deposited_project_cannot_read_outside_its_directory() {
    let out = outside();
    let cases: Vec<(&str, String)> = vec![
        ("env_file", format!("services:\n  web:\n    image: alpine\n    env_file: {}/secret.env\n", out.0.display())),
        ("secrets.file", format!("services:\n  web:\n    image: alpine\n    secrets: [s]\nsecrets:\n  s:\n    file: {}/secret.env\n", out.0.display())),
        ("configs.file", format!("services:\n  web:\n    image: alpine\n    configs: [c]\nconfigs:\n  c:\n    file: {}/secret.env\n", out.0.display())),
        ("bind", format!("services:\n  web:\n    image: alpine\n    volumes:\n      - {}/secret.env:/x:ro\n", out.0.display())),
        ("bind relatif", "services:\n  web:\n    image: alpine\n    volumes:\n      - ../outside/secret.env:/x:ro\n".to_string()),
        ("include", format!("include:\n  - {}/base.yaml\nservices:\n  web:\n    image: alpine\n", out.0.display())),
        ("extends", format!("services:\n  web:\n    extends:\n      file: {}/base.yaml\n      service: base\n", out.0.display())),
        ("build.context", format!("services:\n  web:\n    build: {}\n", out.0.display())),
        ("build.dockerfile", "services:\n  web:\n    build:\n      context: .\n      dockerfile: ../outside/base.yaml\n".to_string()),
    ];
    for (what, compose) in cases {
        let d = Dir::new(&format!("case-{}", what.replace(['.', ' '], "-")));
        d.write("compose.yaml", &compose);
        let confined = codes(&d.0, true);
        assert!(confined.iter().any(|c| c == "path-outside-project"), "{what} : attendu path-outside-project, obtenu {confined:?}");
        let free = codes(&d.0, false);
        assert!(!free.iter().any(|c| c == "path-outside-project"), "{what} sans confinement : {free:?}");
    }
}

#[test]
fn tilde_is_outside_a_deposited_project() {
    let d = Dir::new("tilde");
    d.write("compose.yaml", "services:\n  web:\n    image: alpine\n    env_file: ~/.profile\n");
    assert!(codes(&d.0, true).iter().any(|c| c == "path-outside-project"));
}

#[test]
fn files_inside_the_project_stay_readable() {
    let d = Dir::new("inside");
    d.write("compose.yaml", "include:\n  - sub/other.yaml\nservices:\n  web:\n    image: alpine\n    env_file: ./conf/app.env\n    volumes:\n      - ./conf/app.env:/x:ro\n");
    d.write("conf/app.env", "A=1\n");
    d.write("sub/other.yaml", "services:\n  db:\n    image: alpine\n");
    assert_eq!(codes(&d.0, true), Vec::<String>::new());
}

/// Plan d'un projet déposé : un secret `environment:` lirait
/// l'environnement du serveur ; refusé. Les identifiants Docker du poste
/// n'y entrent pas non plus (`private_registries` vide).
#[test]
fn deposited_project_cannot_read_server_environment() {
    use cc_compose::plan::cli::{run, PlanOptions};
    use cc_compose::plan::images::Fixture;
    let d = Dir::new("secret-env");
    d.write("compose.yaml", "services:\n  web:\n    image: busybox:1.36\n    secrets: [s]\nsecrets:\n  s:\n    environment: HOME\n");
    let fixture: Fixture = serde_json::from_str(r#"{ "busybox:1.36": { "entrypoint": [], "cmd": ["sh"], "user": null, "workdir": null, "exposed_ports": [] } }"#).unwrap();
    let plan_with = |confine: bool| {
        let opts = PlanOptions { project: Options { project_dir: d.0.clone(), files: Vec::new(), project_name: None, env_files: Vec::new(), profiles: Vec::new(), confine }, allow_ephemeral: false, private_registries: None, build_flavor: None };
        let mut diags = Diagnostics::default();
        let plan = run(&opts, &fixture, &mut diags);
        (plan.is_some(), diags.items.iter().map(|x| x.code.to_string()).collect::<Vec<_>>())
    };
    let (ok, codes) = plan_with(true);
    assert!(!ok && codes.iter().any(|c| c == "secret-environment-confined"), "{codes:?}");
    let (ok, codes) = plan_with(false);
    assert!(ok && !codes.iter().any(|c| c == "secret-environment-confined"), "{codes:?}");
}
