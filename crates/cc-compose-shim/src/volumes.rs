//! Volumes éphémères : le point de montage que Docker aurait créé est créé
//! ici, avant l'abandon des privilèges, et donné à l'utilisateur de l'image
//! pour qu'elle puisse y écrire.

use std::path::Path;

use cc_compose_model::shim::ShimConfig;

pub fn prepare(config: &ShimConfig) -> Result<(), String> {
    if config.volumes.is_empty() {
        return Ok(());
    }
    let ids = crate::child::resolve_ids(config)?;
    for target in &config.volumes {
        let path = Path::new(target);
        let existed = path.is_dir();
        std::fs::create_dir_all(path).map_err(|e| format!("volume {target} : création : {e}"))?;
        // Un répertoire déjà dans l'image garde son propriétaire (l'image
        // l'a prévu) ; un répertoire créé ici revient à l'utilisateur.
        if existed {
            continue;
        }
        if let Some(ids) = &ids {
            let c = std::ffi::CString::new(target.as_str()).map_err(|_| format!("volume {target} : chemin invalide"))?;
            // SAFETY: chown sur un chemin C valide.
            if unsafe { libc::chown(c.as_ptr(), ids.uid as libc::uid_t, ids.gid as libc::gid_t) } != 0 {
                return Err(format!("volume {target} : chown : {}", std::io::Error::last_os_error()));
            }
        }
    }
    Ok(())
}
