//! Politique clé par clé : ce que le planificateur ne traduit pas est soit
//! ignoré avec avertissement (sans effet sur la plateforme), soit refusé
//! (sémantique impossible à reproduire), avec le chemin dans le fichier
//! source. Les clés traduites sont consommées dans `plan/mod.rs`.

use crate::compose::model::{Network, Service};
use crate::compose::node::Path;
use crate::diagnostic::{Diagnostic, Diagnostics};

fn refuse_at(diags: &mut Diagnostics, p: &Path, key: &str, why: &str) {
    diags.push(Diagnostic::error("unsupported-key", format!("`{key}` n'a pas d'équivalent sur Clever Cloud : {why}")).with_path(&p.child(key)));
}

fn ignore_at(diags: &mut Diagnostics, p: &Path, key: &str, why: &str) {
    diags.push(Diagnostic::warning("key-ignored", format!("`{key}` est ignoré : {why}")).with_path(&p.child(key)));
}

pub fn service(name: &str, s: &Service, p: &Path, diags: &mut Diagnostics) {
    if s.privileged == Some(true) {
        refuse_at(diags, p, "privileged", "conteneur sans privilèges");
    }
    if !s.cap_add.is_empty() {
        refuse_at(diags, p, "cap_add", "aucune capacité supplémentaire n'est accordée");
    }
    if !s.devices.is_empty() {
        refuse_at(diags, p, "devices", "pas d'accès aux périphériques de l'hôte");
    }
    if let Some(mode) = &s.network_mode {
        refuse_at(diags, p, "network_mode", &format!("`{mode}` : le conteneur tourne déjà en mode hôte sur sa VM, et un réseau Compose devient un network group"));
    }
    if !s.sysctls.is_empty() {
        refuse_at(diags, p, "sysctls", "pas de réglage noyau");
    }
    if !s.security_opt.is_empty() {
        refuse_at(diags, p, "security_opt", "options de sécurité non configurables");
    }
    if s.pid.is_some() {
        refuse_at(diags, p, "pid", "espace de noms PID non partageable");
    }
    if s.ipc.is_some() {
        refuse_at(diags, p, "ipc", "espace de noms IPC non partageable");
    }
    if s.userns_mode.is_some() {
        refuse_at(diags, p, "userns_mode", "espace de noms utilisateur non configurable");
    }
    if s.cgroup.is_some() || s.cgroup_parent.is_some() {
        refuse_at(diags, p, "cgroup", "cgroups non configurables");
    }
    if s.runtime.is_some() {
        refuse_at(diags, p, "runtime", "runtime de conteneur non configurable");
    }
    if !s.volumes_from.is_empty() {
        refuse_at(diags, p, "volumes_from", "pas de volume partagé entre conteneurs");
    }
    if !s.external_links.is_empty() {
        refuse_at(diags, p, "external_links", "pas de conteneur hors projet");
    }
    if !s.links.is_empty() {
        diags.push(Diagnostic::warning("links-ignored", "`links` est obsolète ; les services se joignent par leur nom dans le network group, la dépendance implicite est conservée").with_path(&p.child("links")));
    }
    if s.gpus.is_some() || s.deploy.as_ref().and_then(|d| d.resources.as_ref()).and_then(|r| r.reservations.as_ref()).and_then(|r| r.devices.as_ref()).is_some() {
        refuse_at(diags, p, "gpus", "pas de GPU sur le runtime Docker");
    }
    if s.provider.is_some() {
        refuse_at(diags, p, "provider", "les services fournis par un plugin ne sont pas supportés");
    }
    if s.models.is_some() {
        refuse_at(diags, p, "models", "les modèles Docker Model Runner ne sont pas supportés");
    }
    if s.use_api_socket == Some(true) {
        refuse_at(diags, p, "use_api_socket", "pas d'accès au socket Docker");
    }
    if !s.configs.is_empty() {
        refuse_at(diags, p, "configs", "pas encore supporté (les `secrets` le seront via le shim)");
    }
    if s.ulimits.is_some() {
        ignore_at(diags, p, "ulimits", "le déployeur fixe lui-même `nofile=1024:524288`");
    }
    if s.pids_limit.is_some() {
        ignore_at(diags, p, "pids_limit", "limites de processus non configurables");
    }
    if !s.tmpfs.is_empty() {
        diags.push(Diagnostic::warning("tmpfs-ignored", "`tmpfs` est ignoré : le système de fichiers du conteneur est déjà éphémère").with_path(&p.child("tmpfs")));
    }
    if s.mem_reservation.is_some() || s.memswap_limit.is_some() || s.mem_swappiness.is_some() || s.shm_size.is_some() || s.oom_kill_disable.is_some() || s.oom_score_adj.is_some() {
        diags.push(Diagnostic::warning("memory-tuning-ignored", "réglages mémoire fins (`mem_reservation`, `memswap_limit`, `shm_size`, `oom_*`) ignorés : seule la flavor compte").with_path(p));
    }
    if s.cpus.is_some() || s.cpu_shares.is_some() || s.cpu_count.is_some() || s.cpu_percent.is_some() || s.cpuset.is_some() || s.cpu_quota.is_some() || s.cpu_period.is_some() {
        diags.push(Diagnostic::warning("cpu-tuning-ignored", "réglages CPU ignorés : les CPU viennent de la flavor, choisie par la mémoire").with_path(p));
    }

    // Clés sans effet sur la plateforme : ignorées avec avertissement.
    if s.container_name.is_some() {
        ignore_at(diags, p, "container_name", "le nom du conteneur est celui de l'instance");
    }
    if s.hostname.is_some() || s.domainname.is_some() {
        ignore_at(diags, p, "hostname", "le nom d'hôte est celui de l'instance ; les autres services joignent celui-ci par son nom de service");
    }
    if let Some(r) = &s.restart {
        if r != "always" && r != "unless-stopped" {
            ignore_at(diags, p, "restart", &format!("`{r}` : la plateforme redémarre toujours une application qui s'arrête ; pour un service one-shot, `x-clever.task: true`"));
        }
    }
    if s.logging.is_some() {
        ignore_at(diags, p, "logging", "les logs sont collectés par la plateforme");
    }
    if s.pull_policy.is_some() {
        ignore_at(diags, p, "pull_policy", "l'image est reconstruite à chaque déploiement");
    }
    if let Some(pf) = &s.platform {
        if !pf.contains("amd64") && !pf.contains("x86_64") {
            diags.push(Diagnostic::error("platform-unsupported", format!("`platform: {pf}` : seule amd64 est disponible")).with_path(&p.child("platform")));
        }
    }
    if s.init.is_some() {
        ignore_at(diags, p, "init", "le shim est PID 1 et récupère les zombies");
    }
    if s.tty.is_some() || s.stdin_open.is_some() {
        ignore_at(diags, p, "tty", "pas de terminal attaché");
    }
    if s.read_only.is_some() {
        ignore_at(diags, p, "read_only", "système de fichiers non verrouillable");
    }
    if !s.dns.is_empty() || !s.dns_search.is_empty() {
        ignore_at(diags, p, "dns", "le résolveur est celui de la VM, le shim y ajoute les noms de services");
    }
    if !s.extra_hosts.is_empty() {
        ignore_at(diags, p, "extra_hosts", "pas d'écriture de /etc/hosts (CLAUDE.md §7)");
    }
    if s.stop_grace_period.is_some() || s.stop_signal.is_some() {
        ignore_at(diags, p, "stop_grace_period", "l'arrêt est piloté par la plateforme (SIGTERM relayé par le shim)");
    }
    if !s.cap_drop.is_empty() {
        ignore_at(diags, p, "cap_drop", "aucune capacité n'est accordée de toute façon");
    }
    if !s.labels.is_empty() || !s.annotations.is_empty() {
        ignore_at(diags, p, "labels", "les labels de conteneur n'ont pas de destination ; les tags plateforme sont posés par l'outil");
    }
    if let Some(d) = &s.deploy {
        if d.restart_policy.is_some() || d.update_config.is_some() || d.rollback_config.is_some() || d.placement.is_some() || d.endpoint_mode.is_some() || d.mode.is_some() {
            ignore_at(diags, p, "deploy", "seuls `replicas` et `resources.limits.memory` sont traduits (instances et flavor)");
        }
    }
    if s.develop.is_some() {
        ignore_at(diags, p, "develop", "configuration de développement local");
    }
    if s.attach.is_some() || s.mac_address.is_some() || s.isolation.is_some() || s.uts.is_some() || !s.group_add.is_empty() || s.credential_spec.is_some() || s.blkio_config.is_some() || s.storage_opt.is_some() || s.device_cgroup_rules.is_some() || s.cpu_rt_runtime.is_some() || s.cpu_rt_period.is_some() {
        ignore_at(diags, p, "attach", "options de bas niveau sans équivalent");
    }
    if s.post_start.is_some() || s.pre_stop.is_some() || s.pre_start.is_some() {
        diags.push(Diagnostic::error("lifecycle-hooks-unsupported", "`pre_start`, `post_start` et `pre_stop` ne sont pas supportés").with_path(p));
    }
    if s.label_file.is_some() {
        ignore_at(diags, p, "label_file", "labels sans destination");
    }
    if !s.profiles.is_empty() {
        // Consommé au chargement (profils actifs) : rien à faire ici.
    }
    for (k, _) in &s.extra {
        if !k.starts_with("x-") {
            diags.push(Diagnostic::error("unsupported-key", format!("clé `{k}` non supportée sur le service `{name}`")).with_path(&p.child(k)));
        } else if k != "x-clever" {
            diags.push(Diagnostic::warning("extension-ignored", format!("extension `{k}` ignorée")).with_path(&p.child(k)));
        }
    }
    if let Some(b) = &s.build {
        if b.target.is_some() {
            diags.push(Diagnostic::warning("build-target-ignored", "`build.target` est ignoré : la dernière étape est construite").with_path(&p.child("build").child("target")));
        }
        for (k, _) in &b.extra {
            if !k.starts_with("x-") && k != "args" && k != "labels" && k != "cache_from" && k != "cache_to" && k != "tags" {
                diags.push(Diagnostic::warning("build-key-ignored", format!("`build.{k}` est ignoré")).with_path(&p.child("build").child(k)));
            }
        }
    }
    for (i, pt) in s.ports.iter().enumerate() {
        if pt.host_ip.is_some() {
            diags.push(Diagnostic::warning("port-host-ip-ignored", "l'adresse d'écoute d'un port publié est ignorée").with_path(&p.child("ports").index(i)));
        }
    }
}

pub fn network(name: &str, n: &Network, p: &Path, diags: &mut Diagnostics) {
    if n.external == Some(true) {
        diags.push(Diagnostic::error("network-external", format!("le réseau externe `{name}` n'a pas d'équivalent : un network group est créé par réseau du projet")).with_path(p));
    }
    if let Some(d) = &n.driver {
        if d != "bridge" {
            diags.push(Diagnostic::warning("network-driver-ignored", format!("driver `{d}` ignoré : un réseau devient un network group WireGuard")).with_path(&p.child("driver")));
        }
    }
    if n.ipam.is_some() {
        diags.push(Diagnostic::warning("network-ipam-ignored", "`ipam` est ignoré : le CIDR du network group est attribué par la plateforme").with_path(&p.child("ipam")));
    }
    if n.internal == Some(true) {
        diags.push(Diagnostic::warning("network-internal-ignored", "`internal` est ignoré : un network group n'est joignable que de ses membres, et l'accès public passe par les domaines").with_path(&p.child("internal")));
    }
    for (k, _) in &n.extra {
        if !k.starts_with("x-") {
            diags.push(Diagnostic::warning("network-key-ignored", format!("`{k}` est ignoré sur le réseau `{name}`")).with_path(&p.child(k)));
        }
    }
}
