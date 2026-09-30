//! Écarts entre la plateforme et le compose : ce qui a été changé hors du
//! compose (console, clever-tools) et que `up` écraserait (scaling, build
//! dédié), conserverait sans le dire (variables ajoutées) ou ne saurait pas
//! appliquer (plan d'un addon existant). `pull-config` et l'interface les
//! reprennent dans `x-clever` du compose, sur choix explicite.
//!
//! Aucune valeur de variable ne sort d'ici : une variable reprise devient
//! `NOM: "${NOM:?…}"`, à renseigner dans le `.env` (jamais de secret
//! littéral dans le compose ; sans valeur, le plan refuse au lieu de
//! remplacer la valeur de la console par du vide).

use std::collections::{BTreeMap, BTreeSet};

use cc_compose_model::plan::*;
use serde::Serialize;

use super::platform::Platform;
use super::reconcile::MANAGED_VAR;
use crate::compose::edit::{set_xclever, set_xclever_environment, Entry};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Drift {
    /// Service Compose (celui qui porte `x-clever`).
    pub service: String,
    /// `min_flavor`, `max_flavor`, `min_instances`, `max_instances`,
    /// `build_flavor`, `plan` (addon) ou `environment` (variable ajoutée
    /// hors compose, `name`).
    pub field: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Valeur du plan ; `None` pour une variable (absente du compose) ou un
    /// build dédié absent.
    pub compose: Option<String>,
    /// Valeur servie ; jamais la valeur d'une variable.
    pub platform: Option<String>,
}

impl Drift {
    /// Identifiant stable, pour choisir quoi reprendre.
    pub fn id(&self) -> String {
        match &self.name {
            Some(n) => format!("{}/{}/{n}", self.service, self.field),
            None => format!("{}/{}", self.service, self.field),
        }
    }

    /// Une ligne lisible.
    pub fn describe(&self) -> String {
        let show = |v: &Option<String>| v.clone().unwrap_or_else(|| "aucune".into());
        match self.field.as_str() {
            "environment" => format!("{} : variable `{}` ajoutée hors compose (conservée par `up`, absente du compose)", self.service, self.name.as_deref().unwrap_or("")),
            "plan" => format!("{} : addon au plan `{}` sur la plateforme, `{}` dans le compose (`up` ne migre pas un addon existant)", self.service, show(&self.platform), show(&self.compose)),
            "build_flavor" => format!("{} : instance de build dédiée {} sur la plateforme, {} dans le compose (`up` applique le compose)", self.service, show(&self.platform), show(&self.compose)),
            f => format!("{} : `{f}` vaut {} sur la plateforme, {} dans le compose (`up` applique le compose)", self.service, show(&self.platform), show(&self.compose)),
        }
    }
}

/// Variables posées par l'exécuteur hors du plan (jamais reprises).
fn executor_var(name: &str) -> bool {
    name == MANAGED_VAR || name == "CC_DOCKERFILE" || name.starts_with("CC_DOCKER_LOGIN_")
}

/// Les écarts d'un projet déjà déployé ; un service absent de la plateforme
/// n'en a pas.
pub async fn detect<P: Platform>(plan: &Plan, platform: &P) -> Result<Vec<Drift>, String> {
    let inv = super::inspect::discover(&plan.project, platform).await?;
    let mut out = Vec::new();
    for r in &plan.resources {
        match r {
            Resource::App(app) if app.run == RunWhere::Platform => {
                let Some(pa) = inv.apps.iter().find(|a| a.service == app.service) else { continue };
                let i = &pa.info.instance;
                let mut push = |field: &str, compose: Option<String>, served: Option<String>| {
                    if compose != served {
                        out.push(Drift { service: app.service.clone(), field: field.into(), name: None, compose, platform: served });
                    }
                };
                push("min_flavor", Some(app.scaling.min_flavor.clone()), Some(i.min_flavor.name.clone()));
                push("max_flavor", Some(app.scaling.max_flavor.clone()), Some(i.max_flavor.name.clone()));
                push("min_instances", Some(app.scaling.min_instances.to_string()), Some(i.min_instances.to_string()));
                push("max_instances", Some(app.scaling.max_instances.to_string()), Some(i.max_instances.to_string()));
                push("build_flavor", app.scaling.build_flavor.clone(), pa.info.dedicated_build().map(str::to_string));
                let env = platform.get_env(&pa.info.id).await?;
                let managed: BTreeSet<&str> = env.iter().find(|e| e.name == MANAGED_VAR).map(|e| e.value.split(',').filter(|s| !s.is_empty()).collect()).unwrap_or_default();
                for e in &env {
                    if executor_var(&e.name) || managed.contains(e.name.as_str()) || app.env.iter().any(|p| p.key == e.name) {
                        continue;
                    }
                    out.push(Drift { service: app.service.clone(), field: "environment".into(), name: Some(e.name.clone()), compose: None, platform: None });
                }
            }
            // Un Cellar déclaré n'est pas un service : son plan est unique.
            Resource::Addon(a) if a.provider != "cellar-addon" => {
                let Some(pa) = inv.addons.iter().find(|x| x.service == a.service) else { continue };
                if pa.info.plan_slug != a.plan {
                    out.push(Drift { service: a.service.clone(), field: "plan".into(), name: None, compose: Some(a.plan.clone()), platform: Some(pa.info.plan_slug.clone()) });
                }
            }
            _ => {}
        }
    }
    Ok(out)
}

/// Reprend `drifts` dans le texte du compose. Rend le nouveau texte et les
/// services non repris (défini dans un autre fichier, tâche générée…) avec
/// la raison.
pub fn adopt(text: &str, plan: &Plan, drifts: &[Drift]) -> (String, Vec<(String, String)>) {
    let mut by_service: BTreeMap<&str, Vec<&Drift>> = BTreeMap::new();
    for d in drifts {
        by_service.entry(d.service.as_str()).or_default().push(d);
    }
    let mut text = text.to_string();
    let mut skipped = Vec::new();
    for (service, ds) in by_service {
        match adopt_service(&text, plan, service, &ds) {
            Ok(t) => text = t,
            Err(e) => skipped.push((service.to_string(), e)),
        }
    }
    (text, skipped)
}

fn adopt_service(text: &str, plan: &Plan, service: &str, ds: &[&Drift]) -> Result<String, String> {
    let app = plan.resources.iter().find_map(|r| match r {
        Resource::App(a) if a.service == service => Some(a),
        _ => None,
    });
    let addon = plan.resources.iter().any(|r| matches!(r, Resource::Addon(a) if a.service == service));
    if app.is_none() && !addon {
        return Err("absent du plan".into());
    }
    let has = |f: &str| ds.iter().any(|d| d.field == f);
    let served = |f: &str| ds.iter().find(|d| d.field == f).and_then(|d| d.platform.clone());
    let mut entries: Vec<Entry<'_>> = Vec::new();
    if let Some(app) = app {
        if ["min_flavor", "max_flavor", "min_instances", "max_instances"].iter().any(|f| has(f)) {
            // Le scaling s'écrit en entier : `flavor` quand min = max, les
            // bornes sinon, comme l'éditeur de l'interface.
            let lo = served("min_flavor").unwrap_or_else(|| app.scaling.min_flavor.clone());
            let hi = served("max_flavor").unwrap_or_else(|| app.scaling.max_flavor.clone());
            let same = lo == hi;
            entries.push(Entry { key: "flavor", value: same.then(|| lo.clone()) });
            entries.push(Entry { key: "min_flavor", value: (!same).then(|| lo.clone()) });
            entries.push(Entry { key: "max_flavor", value: (!same).then(|| hi.clone()) });
            entries.push(Entry { key: "min_instances", value: Some(served("min_instances").unwrap_or_else(|| app.scaling.min_instances.to_string())) });
            entries.push(Entry { key: "max_instances", value: Some(served("max_instances").unwrap_or_else(|| app.scaling.max_instances.to_string())) });
        }
        if has("build_flavor") {
            entries.push(Entry { key: "build_flavor", value: Some(served("build_flavor").unwrap_or_else(|| "disabled".into())) });
        }
    }
    if has("plan") {
        entries.push(Entry { key: "plan", value: served("plan") });
    }
    let mut text = if entries.is_empty() { text.to_string() } else { set_xclever(text, service, &entries)? };
    let vars: Vec<Entry<'_>> = ds
        .iter()
        .filter(|d| d.field == "environment")
        .filter_map(|d| d.name.as_deref())
        .map(|n| Entry { key: n, value: Some(format!("\"${{{n}:?variable reprise de la plateforme, à renseigner dans .env}}\"")) })
        .collect();
    if !vars.is_empty() {
        text = set_xclever_environment(&text, service, &vars)?;
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drift(service: &str, field: &str, name: Option<&str>, platform: Option<&str>) -> Drift {
        Drift { service: service.into(), field: field.into(), name: name.map(str::to_string), compose: None, platform: platform.map(str::to_string) }
    }

    #[test]
    fn adopted_values_land_in_x_clever() {
        let text = include_str!("../../tests/corpus/scaling/compose.yaml");
        let plan: Plan = serde_json::from_str(include_str!("../../tests/corpus/scaling/plan.json")).unwrap();
        let drifts = vec![
            drift("auto", "min_flavor", None, Some("M")),
            drift("auto", "build_flavor", None, Some("XL")),
            drift("auto", "environment", Some("FOO"), None),
            drift("fixed", "max_instances", None, Some("5")),
            drift("ghost", "min_instances", None, Some("2")),
        ];
        let (out, skipped) = adopt(text, &plan, &drifts);
        assert_eq!(skipped.len(), 1, "{skipped:?}");
        assert_eq!(skipped[0].0, "ghost");
        // Le scaling s'écrit en entier, les bornes du compose gardées.
        assert!(out.contains("    x-clever:\n      min_flavor: M\n      max_flavor: L\n      min_instances: 1\n      max_instances: 4\n      build_flavor: XL\n      environment:\n        FOO: \"${FOO:?variable reprise de la plateforme, à renseigner dans .env}\"\n"), "{out}");
        // `fixed` n'avait pas de x-clever : la flavor dérivée des ressources devient explicite.
        assert!(out.contains("          cpus: \"2.5\"\n    x-clever:\n      flavor: "), "{out}");
        assert!(out.contains("      min_instances: 3\n      max_instances: 5\n"), "{out}");
    }

    #[test]
    fn dedicated_build_removed_on_the_platform_becomes_disabled() {
        let text = include_str!("../../tests/corpus/build-flavor/compose.yaml");
        let plan: Plan = serde_json::from_str(include_str!("../../tests/corpus/build-flavor/plan.json")).unwrap();
        let (out, skipped) = adopt(text, &plan, &[drift("big", "build_flavor", None, None)]);
        assert!(skipped.is_empty());
        assert!(out.contains("    x-clever:\n      build_flavor: disabled\n  inline:"), "{out}");
    }
}
