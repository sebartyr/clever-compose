//! Contrôle de cohérence du projet typé (`loader/validate.go`,
//! `checkConsistency` de compose-go) : références vers des ressources non
//! déclarées, clés mutuellement exclusives, valeurs contradictoires. Ce que
//! Compose refuse, nous le refusons avec le même sens.

use super::model::Project;
use super::node::Path;
use crate::diagnostic::Diagnostic;

pub fn check(project: &Project) -> Result<(), Diagnostic> {
    for (name, s) in &project.services {
        let p = Path::root().child("services").child(name);
        let err = |code: &'static str, msg: String, path: Path| Err(Diagnostic::error(code, msg).with_path(&path));
        if s.build.is_none() && s.image.is_none() && s.provider.is_none() {
            return err("no-image", format!("le service `{name}` n'a ni `image` ni `build`"), p.clone());
        }
        if let Some(b) = &s.build {
            if b.dockerfile_inline.is_some() && b.dockerfile.is_some() {
                return err("build-exclusive", format!("le service `{name}` déclare `dockerfile` et `dockerfile_inline`, mutuellement exclusifs"), p.child("build"));
            }
        }
        if s.network_mode.is_some() && !s.networks.is_empty() {
            return err("network-mode-exclusive", format!("le service `{name}` déclare `network_mode` et `networks`, mutuellement exclusifs"), p.child("network_mode"));
        }
        for net in s.networks.keys() {
            if !project.networks.contains_key(net) {
                return err("network-undefined", format!("le service `{name}` référence le réseau non déclaré `{net}`"), p.child("networks").child(net));
            }
        }
        if let Some(h) = &s.healthcheck {
            if let Some(first) = h.test.first() {
                if !matches!(first.as_str(), "CMD" | "CMD-SHELL" | "NONE") {
                    return err("healthcheck-test-invalid", "`healthcheck.test` doit commencer par `CMD`, `CMD-SHELL` ou `NONE`".into(), p.child("healthcheck").child("test"));
                }
            }
        }
        for (dep, cfg) in &s.depends_on {
            if !project.services.contains_key(dep) && cfg.required {
                return err("depends-on-unknown", format!("le service `{name}` dépend du service non déclaré `{dep}`"), p.child("depends_on").child(dep));
            }
        }
        if let Some(target) = s.network_mode.as_deref().and_then(|m| m.strip_prefix("service:")) {
            if !project.services.contains_key(target) {
                return err("network-mode-service-unknown", format!("service `{target}` introuvable pour `network_mode: service:{target}`"), p.child("network_mode"));
            }
        }
        for (i, v) in s.volumes.iter().enumerate() {
            if v.kind == "volume" {
                if let Some(src) = v.source.as_deref().filter(|s| !s.is_empty()) {
                    if !project.volumes.contains_key(src) {
                        return err("volume-undefined", format!("le service `{name}` référence le volume non déclaré `{src}`"), p.child("volumes").index(i));
                    }
                }
            }
        }
        for (i, sec) in s.secrets.iter().enumerate() {
            if !project.secrets.contains_key(&sec.source) {
                return err("secret-undefined", format!("le service `{name}` référence le secret non déclaré `{}`", sec.source), p.child("secrets").index(i));
            }
        }
        for (i, c) in s.configs.iter().enumerate() {
            let src = c.get("source").and_then(|x| x.as_str()).unwrap_or("");
            if !project.configs.contains_key(src) {
                return err("config-undefined", format!("le service `{name}` référence la config non déclarée `{src}`"), p.child("configs").index(i));
            }
        }
        let replicas = s.deploy.as_ref().and_then(|d| d.replicas);
        if let (Some(sc), Some(r)) = (s.scale, replicas) {
            if sc != r {
                return err("scale-replicas-conflict", format!("services.{name} : `scale` et `deploy.replicas` ont des valeurs différentes"), p.child("scale"));
            }
        }
        if s.scale.is_some_and(|x| x < 0) || replicas.is_some_and(|x| x < 0) {
            return err("scale-negative", format!("services.{name} : `scale`/`deploy.replicas` doit être positif ou nul"), p.child("scale"));
        }
        if s.container_name.is_some() && s.scale.or(replicas).unwrap_or(1) > 1 {
            return err("container-name-scale", format!("services.{name} : `container_name` est incompatible avec plusieurs réplicas"), p.child("container_name"));
        }
        if let (Some(ml), Some(limit)) = (&s.mem_limit, s.deploy.as_ref().and_then(|d| d.resources.as_ref()).and_then(|r| r.limits.as_ref()).and_then(|l| l.memory.clone())) {
            if super::typed::parse_ram_in_bytes(ml) != super::typed::parse_ram_in_bytes(&limit) {
                return err("mem-limit-conflict", format!("services.{name} : `mem_limit` et `deploy.resources.limits.memory` ont des valeurs différentes"), p.child("mem_limit"));
            }
        }
    }
    Ok(())
}
