//! Matérialisation des secrets : variable d'environnement vers fichier,
//! avant le lancement de l'enfant, puis retrait de la variable.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use cc_compose_model::shim::SecretFile;

/// Écrit chaque secret ; renvoie les variables à retirer de l'environnement
/// de l'enfant.
pub fn materialize(secrets: &[SecretFile]) -> Result<Vec<String>, String> {
    let mut consumed = Vec::new();
    for s in secrets {
        let value = std::env::var_os(&s.env).ok_or_else(|| format!("secret : variable `{}` absente de l'environnement", s.env))?;
        let target = Path::new(&s.target);
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("secret {} : création de {} : {e}", s.target, dir.display()))?;
        }
        // Créé avec son mode final : jamais lisible plus largement, même
        // un instant.
        let mode = s.mode.unwrap_or(0o444);
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(mode).open(target).map_err(|e| format!("secret : écriture de {} : {e}", s.target))?;
            f.write_all(value.as_encoded_bytes()).map_err(|e| format!("secret : écriture de {} : {e}", s.target))?;
        }
        // Le fichier existait peut-être (umask, mode antérieur) : mode imposé.
        std::fs::set_permissions(target, std::fs::Permissions::from_mode(mode)).map_err(|e| format!("secret : mode de {} : {e}", s.target))?;
        if s.uid.is_some() || s.gid.is_some() {
            let c = std::ffi::CString::new(s.target.as_str()).map_err(|_| "secret : chemin invalide".to_string())?;
            // SAFETY: chown sur un chemin C valide ; -1 laisse l'identifiant en place.
            let r = unsafe { libc::chown(c.as_ptr(), s.uid.map(|u| u as libc::uid_t).unwrap_or(u32::MAX), s.gid.map(|g| g as libc::gid_t).unwrap_or(u32::MAX)) };
            if r != 0 {
                return Err(format!("secret : chown de {} : {}", s.target, std::io::Error::last_os_error()));
            }
        }
        consumed.push(s.env.clone());
    }
    Ok(consumed)
}
