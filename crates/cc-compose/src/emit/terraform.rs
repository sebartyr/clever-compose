//! Émetteur Terraform : `main.tf.json`, syntaxe JSON native de Terraform,
//! sans règle d'échappement HCL à gérer, validable par `terraform validate`.
//! Noms de ressources et d'attributs : `data/terraform-provider-schema.json`
//! (provider 2.1.0), relus ici et non mémorisés.
//!
//! Contraintes du provider 2.1.0 (constatées dans le schéma) :
//! - `networkgroups[].fqdn` est requis et ne peut pas référencer l'id de sa
//!   propre ressource : on y pose l'alias `<service>.m.<ng>.cc-ng.cloud`,
//!   accepté par l'API (sans effet DNS aujourd'hui, résolvable si axo#3168
//!   aboutit) ; les références d'environnement calculent le nom dérivé
//!   depuis les ids des autres ressources ;
//! - `redirection` est un bloc unique et TCP : un seul port, pas d'UDP ;
//! - pas d'attribut pour une Clever Task.

use std::path::Path as FsPath;

use cc_compose_model::plan::*;
use serde_json::{json, Map, Value as J};

use super::{ident, member_parts};
use crate::diagnostic::{Diagnostic, Diagnostics};

pub struct Options<'a> {
    /// Répertoire de sortie, pour les chemins `file()` relatifs.
    pub out_dir: &'a FsPath,
    pub project_dir: &'a FsPath,
    /// Valeur par défaut de `var.repository` (URL de `origin` du projet).
    pub repository: Option<String>,
}

pub fn emit(plan: &Plan, opts: &Options<'_>, diags: &mut Diagnostics) -> J {
    let schema = schema();
    let mut variables = Map::new();
    variables.insert("organisation".into(), json!({ "type": "string", "description": "Organisation Clever Cloud (orga_xxx ou user_xxx)", "sensitive": true }));
    let mut repository = json!({ "type": "string", "description": "Dépôt git qui héberge les branches clever-compose/<service> (écrites par `clever-compose export`, à pousser)" });
    if let Some(url) = &opts.repository {
        repository["default"] = json!(url);
    }
    variables.insert("repository".into(), repository);
    let mut networkgroups = Map::new();
    let mut dockers = Map::new();
    let mut addon_resources: Map<String, J> = Map::new(); // type de ressource -> { nom -> corps }

    for r in &plan.resources {
        match r {
            Resource::NetworkGroup(ng) => {
                check_attrs(&schema, "clevercloud_networkgroup", &["name", "tags"], diags);
                networkgroups.insert(ident(&ng.network), json!({ "name": ng.name, "tags": ng.tags }));
            }
            Resource::App(app) => {
                if app.kind == AppKind::Task {
                    diags.push(Diagnostic::warning("terraform-task", format!("`{}` est une tâche : le provider 2.1.0 n'a pas d'attribut pour une Clever Task, la ressource est émise comme une application ordinaire", app.service)));
                }
                check_attrs(&schema, "clevercloud_docker", &["name", "region", "smallest_flavor", "biggest_flavor", "min_instance_count", "max_instance_count", "environment", "networkgroups", "redirection", "vhosts", "dockerfile"], diags);
                let mut res = Map::new();
                res.insert("name".into(), json!(app.name));
                res.insert("region".into(), json!(app.region));
                res.insert("smallest_flavor".into(), json!(app.scaling.min_flavor));
                res.insert("biggest_flavor".into(), json!(app.scaling.max_flavor));
                res.insert("min_instance_count".into(), json!(app.scaling.min_instances));
                res.insert("max_instance_count".into(), json!(app.scaling.max_instances));
                if let Source::Context { .. } = &app.source {
                    res.insert("dockerfile".into(), json!(crate::plan::sources::CONTEXT_DOCKERFILE));
                }
                let mut env = Map::new();
                for e in &app.env {
                    env.insert(e.key.clone(), J::String(template(&e.value, plan, app, opts, &mut variables)));
                }
                if !env.is_empty() {
                    res.insert("environment".into(), J::Object(env));
                }
                let members: Vec<J> = plan
                    .resources
                    .iter()
                    .filter_map(|r| match r {
                        Resource::Membership(m) if m.member == app.id => {
                            let net = m.group.0.strip_prefix("ng/")?;
                            Some(json!({
                                "networkgroup_id": format!("${{clevercloud_networkgroup.{}.id}}", ident(net)),
                                "fqdn": format!("{}.m.${{clevercloud_networkgroup.{}.id}}.cc-ng.cloud", app.service, ident(net)),
                            }))
                        }
                        _ => None,
                    })
                    .collect();
                if !members.is_empty() {
                    res.insert("networkgroups".into(), J::Array(members));
                }
                let redirs: Vec<&Redirection> = plan.resources.iter().filter_map(|r| match r {
                    Resource::Redirection(x) if x.app == app.id => Some(x),
                    _ => None,
                }).collect();
                let mut tcp = redirs.iter().filter(|x| x.protocol == Protocol::Tcp);
                if let Some(first) = tcp.next() {
                    res.insert("redirection".into(), json!({ "namespace": first.namespace }));
                }
                for u in redirs.iter().filter(|x| x.protocol == Protocol::Udp) {
                    diags.push(Diagnostic::warning("terraform-udp", format!("`{}` : la redirection UDP du port {} n'est pas exprimable avec le provider 2.1.0 (bloc `redirection` TCP seulement) ; à créer à la main", app.service, u.target_port)));
                }
                let vhosts: Vec<J> = plan.resources.iter().filter_map(|r| match r {
                    Resource::Domain(d) if d.app == app.id => Some(json!({ "fqdn": d.fqdn })),
                    _ => None,
                }).collect();
                if !vhosts.is_empty() {
                    res.insert("vhosts".into(), J::Array(vhosts));
                }
                res.insert("deployment".into(), json!({ "repository": "${var.repository}", "commit": format!("refs/heads/clever-compose/{}", app.service) }));
                let deps: Vec<J> = app.depends_on.iter().filter_map(|d| d.0.strip_prefix("app/")).map(|s| json!(format!("clevercloud_docker.{}", ident(s)))).collect();
                if !deps.is_empty() {
                    res.insert("depends_on".into(), J::Array(deps));
                }
                // Addons liés : `dependencies` (ids d'addons), ce qui injecte
                // leurs variables dans l'application.
                let addon_deps: Vec<J> = app
                    .depends_on
                    .iter()
                    .filter_map(|d| d.0.strip_prefix("addon/"))
                    .filter_map(|svc| {
                        let provider = plan.resources.iter().find_map(|r| match r {
                            Resource::Addon(a) if a.service == svc => Some(a.provider.clone()),
                            _ => None,
                        })?;
                        let resource = addon_resource_type(&provider)?;
                        Some(json!(format!("${{{resource}.{}.id}}", ident(svc))))
                    })
                    .collect();
                if !addon_deps.is_empty() {
                    check_attrs(&schema, "clevercloud_docker", &["dependencies"], diags);
                    res.insert("dependencies".into(), J::Array(addon_deps));
                }
                dockers.insert(ident(&app.service), J::Object(res));
            }
            Resource::Addon(a) => {
                let Some(resource) = addon_resource_type(&a.provider) else {
                    diags.push(Diagnostic::warning("terraform-addon", format!("addon `{}` : pas de ressource Terraform pour `{}` dans le provider 2.1.0", a.name, a.provider)));
                    continue;
                };
                let has_version = schema["provider_schemas"]["registry.terraform.io/clevercloud/clevercloud"]["resource_schemas"][resource]["block"]["attributes"].get("version").is_some();
                check_attrs(&schema, resource, &["name", "plan", "region"], diags);
                let mut body = json!({ "name": a.name, "plan": a.plan, "region": a.region });
                if let (Some(v), true) = (&a.version, has_version) {
                    body["version"] = json!(v);
                }
                addon_resources.entry(resource.to_string()).or_insert_with(|| J::Object(Map::new())).as_object_mut().unwrap().insert(ident(&a.service), body);
            }
            Resource::Membership(_) | Resource::Redirection(_) | Resource::Domain(_) => {}
        }
    }

    let mut resources = Map::new();
    if !networkgroups.is_empty() {
        resources.insert("clevercloud_networkgroup".into(), J::Object(networkgroups));
    }
    for (k, v) in addon_resources {
        resources.insert(k, v);
    }
    if !dockers.is_empty() {
        resources.insert("clevercloud_docker".into(), J::Object(dockers));
    }
    json!({
        "terraform": { "required_providers": { "clevercloud": { "source": "CleverCloud/clevercloud", "version": "~> 2.1" } } },
        "provider": { "clevercloud": { "organisation": "${var.organisation}" } },
        "variable": variables,
        "resource": resources,
    })
}

/// Un template du plan en chaîne Terraform : littéraux échappés (`$` et
/// `%` doublés), références interpolées.
fn template(t: &Template, plan: &Plan, app: &App, opts: &Options<'_>, variables: &mut Map<String, J>) -> String {
    let mut out = String::new();
    for seg in &t.0 {
        match seg {
            Segment::Literal(s) => out.push_str(&s.replace('$', "$$").replace('%', "%%")),
            Segment::Ref(Reference::MemberFqdn { member }) => match member_parts(plan, &member.0) {
                Some((service, net)) => out.push_str(&format!("${{clevercloud_docker.{}.id}}.m.${{clevercloud_networkgroup.{}.id}}.cc-ng.cloud", ident(&service), ident(&net))),
                None => out.push_str(&format!("<{}>", member.0)),
            },
            // Expansée par le shim au démarrage : reste littérale.
            Segment::Ref(Reference::AddonEnv { var, .. }) => out.push_str(&format!("$${{{var}}}")),
            Segment::Ref(Reference::Secret { name }) => {
                let binding = app.secrets.iter().find(|s| &s.name == name);
                match binding.map(|b| &b.source) {
                    Some(SecretSource::File { path }) => {
                        let abs = opts.project_dir.join(path.trim_start_matches("./"));
                        let rel = pathdiff(&abs, opts.out_dir);
                        out.push_str(&format!("${{file(\"${{path.module}}/{rel}\")}}"));
                    }
                    Some(SecretSource::Environment { var }) => {
                        let vname = format!("secret_{}", ident(&var.to_lowercase()));
                        variables.entry(vname.clone()).or_insert(json!({ "type": "string", "sensitive": true, "description": format!("Valeur du secret `{name}` (variable d'environnement {var} au moment du up)") }));
                        out.push_str(&format!("${{var.{vname}}}"));
                    }
                    None => out.push_str(&format!("<secret {name}>")),
                }
            }
        }
    }
    out
}

/// Chemin relatif de `target` depuis `base` (POSIX, sans résolution de liens).
pub fn pathdiff(target: &FsPath, base: &FsPath) -> String {
    let t: Vec<_> = target.components().collect();
    let b: Vec<_> = base.components().collect();
    let common = t.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut parts: Vec<String> = vec!["..".into(); b.len() - common];
    parts.extend(t[common..].iter().map(|c| c.as_os_str().to_string_lossy().to_string()));
    parts.join("/")
}

/// Ressource Terraform d'un fournisseur d'addon.
fn addon_resource_type(provider: &str) -> Option<&'static str> {
    Some(match provider {
        "postgresql-addon" => "clevercloud_postgresql",
        "mysql-addon" => "clevercloud_mysql",
        "redis-addon" => "clevercloud_redis",
        "mongodb-addon" => "clevercloud_mongodb",
        "config-provider" => "clevercloud_configprovider",
        _ => return None,
    })
}

pub fn schema() -> J {
    serde_json::from_str(include_str!("../../data/terraform-provider-schema.json")).expect("schéma du provider invalide")
}

/// Garde-fou : chaque attribut émis existe dans le schéma embarqué.
fn check_attrs(schema: &J, resource: &str, attrs: &[&str], diags: &mut Diagnostics) {
    let block = &schema["provider_schemas"]["registry.terraform.io/clevercloud/clevercloud"]["resource_schemas"][resource]["block"];
    for a in attrs {
        if block["attributes"].get(*a).is_none() && block["block_types"].get(*a).is_none() {
            diags.push(Diagnostic::error("terraform-schema", format!("attribut `{a}` absent de `{resource}` dans le schéma embarqué du provider : émetteur à mettre à jour")));
        }
    }
}
