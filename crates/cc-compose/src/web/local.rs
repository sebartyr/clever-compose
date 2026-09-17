//! Projets locaux connus du serveur : un répertoire (permanent) ou une
//! archive extraite dans un répertoire temporaire, supprimé à la fin.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct LocalProject {
    pub id: String,
    pub dir: PathBuf,
    /// Répertoire créé par le serveur (archive) : supprimé à l'oubli.
    pub temporary: bool,
    /// Utilisateur Clever Cloud (id) qui l'a déposé ; `None` : projet du
    /// serveur lui-même (répertoire de lancement, client local sans
    /// jetons), visible de tous.
    pub owner: Option<String>,
}

fn visible(p: &LocalProject, owner: Option<&str>) -> bool {
    p.owner.is_none() || p.owner.as_deref() == owner
}

#[derive(Default)]
pub struct Locals {
    inner: Mutex<BTreeMap<String, LocalProject>>,
    next: std::sync::atomic::AtomicUsize,
}

impl Locals {
    fn new_id(&self, dir: &Path) -> String {
        let n = self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let base = dir.file_name().and_then(|s| s.to_str()).unwrap_or("projet");
        let safe: String = base.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
        format!("{safe}-{n}")
    }

    pub fn add_dir(&self, dir: PathBuf, temporary: bool, owner: Option<&str>) -> Result<String, String> {
        // `~` et chemins relatifs au lancement, comme dans un shell.
        let dir = match dir.to_str().and_then(|s| s.strip_prefix("~/")) {
            Some(rest) => std::env::var_os("HOME").map(|h| PathBuf::from(h).join(rest)).unwrap_or(dir),
            None => dir,
        };
        let dir = std::fs::canonicalize(&dir).map_err(|e| format!("{} : {e}", dir.display()))?;
        if !dir.is_dir() {
            return Err(format!("{} n'est pas un répertoire", dir.display()));
        }
        if !temporary && !["compose.yaml", "compose.yml", "docker-compose.yaml", "docker-compose.yml"].iter().any(|f| dir.join(f).is_file()) {
            return Err(format!("{} : aucun compose.yaml / docker-compose.yml", dir.display()));
        }
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = inner.values().find(|p| p.dir == dir && visible(p, owner)) {
            return Ok(p.id.clone());
        }
        let id = self.new_id(&dir);
        inner.insert(id.clone(), LocalProject { id: id.clone(), dir, temporary, owner: owner.map(str::to_string) });
        Ok(id)
    }

    /// Extrait une archive (`.zip`, `.tar.gz`, `.tgz`, `.tar`) dans un
    /// répertoire temporaire et l'enregistre. Si l'archive contient un seul
    /// répertoire racine, c'est lui le projet.
    pub fn add_archive(&self, name: &str, bytes: &[u8], owner: Option<&str>) -> Result<String, String> {
        let stem = Path::new(name).file_name().and_then(|s| s.to_str()).unwrap_or("archive").trim_end_matches(".zip").trim_end_matches(".tgz").trim_end_matches(".tar.gz").trim_end_matches(".tar").to_string();
        let safe_stem: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' }).collect();
        let root = fresh_temp_dir(self, "archive")?;
        let target = root.join(if safe_stem.is_empty() { "archive" } else { safe_stem.as_str() });
        std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
        let lower = name.to_ascii_lowercase();
        let extracted = if lower.ends_with(".zip") {
            extract_zip(bytes, &target)
        } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
            extract_tar(flate2::read::GzDecoder::new(bytes), &target)
        } else if lower.ends_with(".tar") {
            extract_tar(bytes, &target)
        } else {
            Err(format!("`{name}` : archive .zip, .tar.gz, .tgz ou .tar attendue"))
        };
        if let Err(e) = extracted {
            let _ = std::fs::remove_dir_all(&root);
            return Err(e);
        }
        // Un seul répertoire à la racine, sans compose à côté : on y entre.
        let mut dir = target;
        loop {
            let entries: Vec<PathBuf> = std::fs::read_dir(&dir).map_err(|e| e.to_string())?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.file_name().and_then(|n| n.to_str()).map(|n| !n.starts_with("__MACOSX") && n != ".DS_Store").unwrap_or(true)).collect();
            match entries.as_slice() {
                [only] if only.is_dir() => dir = only.clone(),
                _ => break,
            }
        }
        self.add_dir(dir, true, owner)
    }

    /// Le répertoire d'un projet, s'il est visible de `owner`, et s'il
    /// est **déposé** (archive, assets rapatriés, ou envoyé par un client
    /// à jetons) : ses fichiers sont alors confinés à son répertoire.
    pub fn get(&self, id: &str, owner: Option<&str>) -> Option<(PathBuf, bool)> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).get(id).filter(|p| visible(p, owner)).map(|p| (p.dir.clone(), p.temporary || p.owner.is_some()))
    }

    pub fn remove(&self, id: &str, owner: Option<&str>) -> bool {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if !inner.get(id).is_some_and(|p| visible(p, owner)) {
            return false;
        }
        let Some(p) = inner.remove(id) else { return false };
        drop(inner);
        if p.temporary {
            // Le répertoire à supprimer est la racine créée par
            // `fresh_temp_dir` (un ancêtre `clever-compose-ui-…`) ; sans
            // elle, rien n'est supprimé : jamais de remontée vers `/`.
            let root = p.dir.ancestors().find(|a| a.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(TEMP_PREFIX)));
            if let Some(root) = root {
                let _ = std::fs::remove_dir_all(root);
            }
        }
        true
    }

    pub fn list(&self, owner: Option<&str>) -> Vec<(String, PathBuf, bool)> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).values().filter(|p| visible(p, owner)).map(|p| (p.id.clone(), p.dir.clone(), p.temporary)).collect()
    }
}

const TEMP_PREFIX: &str = "clever-compose-ui-";

/// Un répertoire temporaire neuf, propre à ce serveur et à cette
/// opération (`clever-compose-ui-<pid>-<n>-<quoi>`), créé en 0700 :
/// jamais réutilisé, jamais partagé entre deux dépôts.
pub fn fresh_temp_dir(locals: &Locals, what: &str) -> Result<PathBuf, String> {
    let n = locals.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!("{TEMP_PREFIX}{}-{n}-{what}", std::process::id()));
    if root.exists() {
        return Err(format!("{} existe déjà", root.display()));
    }
    std::fs::create_dir_all(&root).map_err(|e| format!("{} : {e}", root.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700));
    }
    Ok(root)
}

/// Un chemin d'archive ramené sous `target`, sans sortie possible.
fn safe_join(target: &Path, rel: &str) -> Option<PathBuf> {
    let mut out = target.to_path_buf();
    for c in Path::new(rel).components() {
        match c {
            std::path::Component::Normal(n) => out.push(n),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    Some(out)
}

/// Bornes d'extraction d'une archive déposée : une archive de quelques Kio
/// peut décompresser en Gio (zip bomb). Taille décompressée cumulée,
/// taille par fichier, nombre d'entrées.
pub const MAX_EXTRACTED: u64 = 512 * 1024 * 1024;
pub const MAX_FILE: u64 = 64 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 20_000;

/// Compteurs d'extraction, vérifiés entrée par entrée.
struct Budget {
    extracted: u64,
    entries: usize,
}

impl Budget {
    fn new() -> Budget {
        Budget { extracted: 0, entries: 0 }
    }

    fn entry(&mut self) -> Result<(), String> {
        self.entries += 1;
        if self.entries > MAX_ENTRIES {
            return Err(format!("archive : plus de {MAX_ENTRIES} entrées"));
        }
        Ok(())
    }

    /// Copie `reader` dans `path` en flux, en s'arrêtant dès qu'une borne
    /// est dépassée (le fichier partiel est retiré).
    fn write<R: Read>(&mut self, mut reader: R, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let allowed = MAX_FILE.min(MAX_EXTRACTED.saturating_sub(self.extracted));
        let mut out = std::fs::File::create(path).map_err(|e| format!("{} : {e}", path.display()))?;
        let written = std::io::copy(&mut (&mut reader).take(allowed), &mut out).map_err(|e| format!("{} : {e}", path.display()))?;
        self.extracted += written;
        // Il restait des octets : borne atteinte.
        let mut probe = [0u8; 1];
        if written == allowed && reader.read(&mut probe).map_err(|e| e.to_string())? > 0 {
            drop(out);
            let _ = std::fs::remove_file(path);
            return Err(format!("archive : {} dépasse {} Mio par fichier ou {} Mio au total", path.file_name().and_then(|n| n.to_str()).unwrap_or("?"), MAX_FILE / (1024 * 1024), MAX_EXTRACTED / (1024 * 1024)));
        }
        Ok(())
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode & 0o777));
}

fn extract_zip(bytes: &[u8], target: &Path) -> Result<(), String> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("zip : {e}"))?;
    let mut budget = Budget::new();
    for i in 0..z.len() {
        budget.entry()?;
        let f = z.by_index(i).map_err(|e| format!("zip : {e}"))?;
        let Some(path) = safe_join(target, f.name()) else { continue };
        if f.is_dir() {
            std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            continue;
        }
        let mode = f.unix_mode();
        budget.write(f, &path)?;
        #[cfg(unix)]
        if let Some(mode) = mode {
            set_mode(&path, mode);
        }
    }
    Ok(())
}

fn extract_tar<R: Read>(reader: R, target: &Path) -> Result<(), String> {
    let mut a = tar::Archive::new(reader);
    let mut budget = Budget::new();
    for entry in a.entries().map_err(|e| format!("tar : {e}"))? {
        budget.entry()?;
        let mut e = entry.map_err(|e| format!("tar : {e}"))?;
        let rel = e.path().map_err(|e| e.to_string())?.to_string_lossy().to_string();
        let Some(path) = safe_join(target, &rel) else { continue };
        match e.header().entry_type() {
            tar::EntryType::Directory => {
                std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            }
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                let mode = e.header().mode().ok();
                budget.write(&mut e, &path)?;
                #[cfg(unix)]
                if let Some(mode) = mode {
                    set_mode(&path, mode);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Retirer un projet temporaire ne supprime que la racine créée par
    /// `fresh_temp_dir` ; un répertoire enregistré temporaire sans cette
    /// racine (jamais le cas en pratique) n'est pas touché, et surtout
    /// rien au-dessus.
    #[test]
    fn remove_only_deletes_our_temp_root() {
        let locals = Locals::default();
        let root = fresh_temp_dir(&locals, "test").unwrap();
        let project = root.join("p");
        std::fs::create_dir_all(&project).unwrap();
        let id = locals.add_dir(project.clone(), true, None).unwrap();
        assert!(locals.remove(&id, None));
        assert!(!root.exists());

        let foreign = std::env::temp_dir().join(format!("cc-compose-foreign-{}", std::process::id()));
        std::fs::create_dir_all(&foreign).unwrap();
        let id = locals.add_dir(foreign.clone(), true, None).unwrap();
        assert!(locals.remove(&id, None));
        assert!(foreign.exists(), "un répertoire hors de nos racines n'est pas supprimé");
        let _ = std::fs::remove_dir_all(&foreign);
    }

    /// Une archive qui décompresse au-delà des bornes est refusée et ne
    /// laisse rien derrière elle.
    #[test]
    fn oversized_archive_is_refused_and_cleaned() {
        use std::io::Write;
        let locals = Locals::default();
        // zip : un fichier de MAX_FILE + 1 octets de zéros (quelques Kio compressés).
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            w.start_file("compose.yaml", zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(b"services: {}\n").unwrap();
            w.start_file("big.bin", zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)).unwrap();
            let chunk = vec![0u8; 1024 * 1024];
            for _ in 0..(MAX_FILE / (1024 * 1024)) {
                w.write_all(&chunk).unwrap();
            }
            w.write_all(&[0]).unwrap();
            w.finish().unwrap();
        }
        let before: Vec<PathBuf> = std::fs::read_dir(std::env::temp_dir()).unwrap().filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(TEMP_PREFIX))).collect();
        let err = locals.add_archive("bomb.zip", buf.get_ref(), None).unwrap_err();
        assert!(err.contains("dépasse"), "{err}");
        let after: Vec<PathBuf> = std::fs::read_dir(std::env::temp_dir()).unwrap().filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(TEMP_PREFIX))).collect();
        assert_eq!(before, after, "la racine temporaire de l'archive refusée est supprimée");

        // tar : trop d'entrées.
        let mut tarbuf = Vec::new();
        {
            let mut t = tar::Builder::new(&mut tarbuf);
            for i in 0..=MAX_ENTRIES {
                let mut h = tar::Header::new_gnu();
                h.set_size(1);
                h.set_mode(0o644);
                h.set_cksum();
                t.append_data(&mut h, format!("f{i}"), &b"x"[..]).unwrap();
            }
            t.finish().unwrap();
        }
        let err = locals.add_archive("many.tar", &tarbuf, None).unwrap_err();
        assert!(err.contains("entrées"), "{err}");
    }
}
