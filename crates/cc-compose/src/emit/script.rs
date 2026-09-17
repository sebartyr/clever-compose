//! Émetteur clever-tools : un script shell de bootstrap à usage unique,
//! fondamentalement non idempotent (CLAUDE.md §10). Terraform est l'export
//! réconciliable ; ce script sert à comprendre ou à reprendre la main une
//! fois. Séquence : groupes, applications sans déploiement, liaisons,
//! variables (les FQDN étant connus dès la création), redirections,
//! domaines, puis déploiements dans l'ordre topologique.

use std::fmt::Write;

use cc_compose_model::plan::*;

use super::{ident, member_parts, MemberKind};
use crate::diagnostic::{Diagnostic, Diagnostics};

/// Citation shell d'un littéral : simples guillemets, `'` échappé.
fn sq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub struct Options<'a> {
    pub out_dir: &'a std::path::Path,
    pub project_dir: &'a std::path::Path,
}

fn template(t: &Template, plan: &Plan, app: &App, project_rel: &str) -> String {
    let mut out = String::new();
    for seg in &t.0 {
        match seg {
            Segment::Literal(s) => out.push_str(&sq(s)),
            Segment::Ref(Reference::MemberFqdn { member }) => match member_parts(plan, &member.0) {
                Some((MemberKind::App, service, net)) => out.push_str(&format!("\"${{APP_{}}}.m.${{NG_{}}}.cc-ng.cloud\"", ident(&service), ident(&net))),
                Some((MemberKind::Addon, service, net)) => out.push_str(&format!("\"${{ADDON_REAL_{}}}.m.${{NG_{}}}.cc-ng.cloud\"", ident(&service), ident(&net))),
                None => out.push_str(&sq(&format!("<{}>", member.0))),
            },
            // Expansée par le shim au démarrage : littérale.
            Segment::Ref(Reference::AddonEnv { var, .. }) => out.push_str(&sq(&format!("${{{var}}}"))),
            Segment::Ref(Reference::Secret { name }) => match app.secrets.iter().find(|s| &s.name == name).map(|s| &s.source) {
                Some(SecretSource::File { path }) => out.push_str(&format!("\"$(cat {})\"", sq(&format!("{project_rel}/{}", path.trim_start_matches("./"))))),
                Some(SecretSource::Environment { var }) => out.push_str(&format!("\"${{{var}:?variable {var} requise pour le secret {name}}}\"")),
                None => out.push_str(&sq(&format!("<secret {name}>"))),
            },
        }
    }
    if out.is_empty() {
        out.push_str("''");
    }
    out
}

pub fn emit(plan: &Plan, opts: &Options<'_>, diags: &mut Diagnostics) -> String {
    let project_rel = super::terraform::pathdiff(opts.project_dir, opts.out_dir);
    let project_rel = if project_rel.is_empty() { ".".to_string() } else { project_rel };
    let mut s = String::new();
    let _ = writeln!(s, "#!/usr/bin/env bash");
    let _ = writeln!(s, "# Bootstrap généré par clever-compose pour le projet `{}` (spec {}).", plan.project, plan.spec_hash);
    let _ = writeln!(s, "# À usage unique : ce script crée les ressources, il ne les réconcilie pas.");
    let _ = writeln!(s, "# Les dépôts synthétiques sont attendus dans ./repos/<service> (export).");
    let _ = writeln!(s, "set -euo pipefail");
    let _ = writeln!(s, "ORG=\"${{CLEVER_COMPOSE_ORG:?organisation Clever Cloud requise (orga_xxx ou user_xxx)}}\"");
    let _ = writeln!(s, "cd \"$(dirname \"$0\")\"");
    let _ = writeln!(s);

    let groups: Vec<&NetworkGroup> = plan.resources.iter().filter_map(|r| if let Resource::NetworkGroup(g) = r { Some(g) } else { None }).collect();
    let apps: Vec<&App> = plan.resources.iter().filter_map(|r| if let Resource::App(a) = r { Some(a) } else { None }).collect();

    if !groups.is_empty() {
        let _ = writeln!(s, "# Network groups (un par réseau Compose)");
        let _ = writeln!(s, "clever features enable ng >/dev/null 2>&1 || true");
        for g in &groups {
            let _ = writeln!(s, "clever ng create {} -o \"$ORG\" --tags {}", sq(&g.name), sq(&g.tags.join(",")));
            let _ = writeln!(s, "NG_{}=\"$(clever ng get {} -o \"$ORG\" -F json | jq -r .id)\"", ident(&g.network), sq(&g.name));
        }
        let _ = writeln!(s);
    }

    let addons: Vec<&Addon> = plan.resources.iter().filter_map(|r| if let Resource::Addon(a) = r { Some(a) } else { None }).collect();
    if !addons.is_empty() {
        let _ = writeln!(s, "# Addons managés (facturés selon le plan)");
        for a in &addons {
            let version = a.version.as_ref().map(|v| format!(" --addon-version {}", sq(v))).unwrap_or_default();
            let options: String = a.options.iter().map(|(k, v)| format!(" --option {}", sq(&format!("{k}={v}")))).collect();
            let _ = writeln!(s, "clever addon create {} {} --plan {} --region {}{}{} -o \"$ORG\"", sq(&a.provider), sq(&a.name), sq(&a.plan), sq(&a.region), version, options);
            let _ = writeln!(s, "ADDON_{}=\"$(clever addon list -o \"$ORG\" -F json | jq -r '.[] | select(.name == {}) | .addonId')\"", ident(&a.service), sq(&a.name).replace('\'', "\""));
            if a.private {
                let _ = writeln!(s, "ADDON_REAL_{}=\"$(clever addon list -o \"$ORG\" -F json | jq -r '.[] | select(.name == {}) | .realId')\"", ident(&a.service), sq(&a.name).replace('\'', "\""));
            }
        }
        let _ = writeln!(s);
    }

    let _ = writeln!(s, "# Applications, créées sans déploiement");
    for a in &apps {
        let task = if a.kind == AppKind::Task { " --task ''" } else { "" };
        if a.kind == AppKind::Task {
            diags.push(Diagnostic::warning("script-task", format!("`{}` est une tâche : `clever create --task` attend la commande, laissée vide puisque le shim la porte ; vérifiez le résultat", a.service)));
        }
        let _ = writeln!(s, "APP_{}=\"$(clever create -t docker {} -o \"$ORG\" -r {}{task} -F json | jq -r .id)\"", ident(&a.service), sq(&a.name), sq(&a.region));
        let _ = writeln!(s, "clever scale --app \"$APP_{}\" --min-flavor {} --max-flavor {} --min-instances {} --max-instances {}", ident(&a.service), sq(&a.scaling.min_flavor), sq(&a.scaling.max_flavor), a.scaling.min_instances, a.scaling.max_instances);
        if let Source::Context { .. } = a.source {
            let _ = writeln!(s, "clever env set --app \"$APP_{}\" CC_DOCKERFILE {}", ident(&a.service), sq(crate::plan::sources::CONTEXT_DOCKERFILE));
        }
        for t in &a.tags {
            let _ = writeln!(s, "# tag : {t}");
        }
    }
    let _ = writeln!(s);

    let memberships: Vec<&Membership> = plan.resources.iter().filter_map(|r| if let Resource::Membership(m) = r { Some(m) } else { None }).collect();
    if !memberships.is_empty() {
        let _ = writeln!(s, "# Liaisons aux network groups");
        for m in &memberships {
            let Some(net) = m.group.0.strip_prefix("ng/") else { continue };
            let group = groups.iter().find(|g| g.network == net).map(|g| g.name.as_str()).unwrap_or(net);
            // Une application par son id ; un addon par son id réel (doc : les
            // network groups attendent `postgresql_xxx`, pas `addon_xxx`).
            if let Some(service) = m.member.0.strip_prefix("app/") {
                let _ = writeln!(s, "clever ng link \"$APP_{}\" {} -o \"$ORG\"", ident(service), sq(group));
            } else if let Some(service) = m.member.0.strip_prefix("addon/") {
                let _ = writeln!(s, "clever ng link \"$ADDON_REAL_{}\" {} -o \"$ORG\"", ident(service), sq(group));
            }
        }
        let _ = writeln!(s);
    }

    if !addons.is_empty() {
        let _ = writeln!(s, "# Liaisons aux addons (injection de leurs variables)");
        for a in &apps {
            for dep in a.depends_on.iter().filter_map(|d| d.0.strip_prefix("addon/")) {
                let _ = writeln!(s, "clever service link-addon \"$ADDON_{}\" --app \"$APP_{}\"", ident(dep), ident(&a.service));
            }
        }
        let _ = writeln!(s);
    }

    let _ = writeln!(s, "# Variables d'environnement (les FQDN des membres sont connus dès la création)");
    for a in &apps {
        for e in &a.env {
            let _ = writeln!(s, "clever env set --app \"$APP_{}\" {} {}", ident(&a.service), sq(&e.key), template(&e.value, plan, a, &project_rel));
        }
    }
    let _ = writeln!(s);

    let redirs: Vec<&Redirection> = plan.resources.iter().filter_map(|r| if let Resource::Redirection(x) = r { Some(x) } else { None }).collect();
    if !redirs.is_empty() {
        let _ = writeln!(s, "# Redirections (port externe attribué par la plateforme, affiché par la commande)");
        for r in &redirs {
            let Some(service) = r.app.0.strip_prefix("app/") else { continue };
            match r.protocol {
                Protocol::Tcp => {
                    let _ = writeln!(s, "clever tcp-redirs add --app \"$APP_{}\" --namespace {}", ident(service), sq(&r.namespace));
                }
                Protocol::Udp => {
                    let _ = writeln!(s, "# UDP {} : pas de commande clever-tools ; API POST .../applications/$APP_{}/udpRedirs", r.target_port, ident(service));
                    diags.push(Diagnostic::warning("script-udp", format!("`{service}` : la redirection UDP n'a pas de commande clever-tools, laissée en commentaire")));
                }
            }
        }
        let _ = writeln!(s);
    }

    let domains: Vec<&Domain> = plan.resources.iter().filter_map(|r| if let Resource::Domain(d) = r { Some(d) } else { None }).collect();
    if !domains.is_empty() {
        let _ = writeln!(s, "# Domaines");
        for d in &domains {
            let Some(service) = d.app.0.strip_prefix("app/") else { continue };
            let _ = writeln!(s, "clever domain add {} --app \"$APP_{}\"", sq(&d.fqdn), ident(service));
        }
        let _ = writeln!(s);
    }

    let _ = writeln!(s, "# Déploiements, dans l'ordre topologique (dépôts synthétiques exportés)");
    for a in &apps {
        let _ = writeln!(s, "(");
        let _ = writeln!(s, "  cd repos/{}", sq(&a.service));
        let _ = writeln!(s, "  [ -d .git ] || git init -q -b main");
        let _ = writeln!(s, "  git add -A");
        let _ = writeln!(s, "  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'");
        let _ = writeln!(s, "  clever deploy --app \"$APP_{}\"", ident(&a.service));
        let _ = writeln!(s, ")");
    }
    s
}
