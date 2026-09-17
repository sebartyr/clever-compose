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
        let mut inner = self.inner.lock().unwrap();
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
        let root = std::env::temp_dir().join(format!("clever-compose-ui-{}-{}", std::process::id(), self.next.load(std::sync::atomic::Ordering::SeqCst)));
        let target = root.join(&stem);
        std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".zip") {
            extract_zip(bytes, &target)?;
        } else if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
            let gz = flate2::read::GzDecoder::new(bytes);
            extract_tar(gz, &target)?;
        } else if lower.ends_with(".tar") {
            extract_tar(bytes, &target)?;
        } else {
            let _ = std::fs::remove_dir_all(&root);
            return Err(format!("`{name}` : archive .zip, .tar.gz, .tgz ou .tar attendue"));
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
        self.inner.lock().unwrap().get(id).filter(|p| visible(p, owner)).map(|p| (p.dir.clone(), p.temporary || p.owner.is_some()))
    }

    pub fn remove(&self, id: &str, owner: Option<&str>) -> bool {
        let mut inner = self.inner.lock().unwrap();
        if !inner.get(id).is_some_and(|p| visible(p, owner)) {
            return false;
        }
        let Some(p) = inner.remove(id) else { return false };
        drop(inner);
        if p.temporary {
            // Le répertoire temporaire est le parent créé par `add_archive`.
            let mut root = p.dir.clone();
            while let Some(parent) = root.parent() {
                if parent.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("clever-compose-ui-")) {
                    root = parent.to_path_buf();
                    break;
                }
                if parent == std::env::temp_dir() {
                    break;
                }
                root = parent.to_path_buf();
            }
            let _ = std::fs::remove_dir_all(root);
        }
        true
    }

    pub fn list(&self, owner: Option<&str>) -> Vec<(String, PathBuf, bool)> {
        self.inner.lock().unwrap().values().filter(|p| visible(p, owner)).map(|p| (p.id.clone(), p.dir.clone(), p.temporary)).collect()
    }
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

fn extract_zip(bytes: &[u8], target: &Path) -> Result<(), String> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| format!("zip : {e}"))?;
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(|e| format!("zip : {e}"))?;
        let Some(path) = safe_join(target, f.name()) else { continue };
        if f.is_dir() {
            std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut content = Vec::new();
        f.read_to_end(&mut content).map_err(|e| e.to_string())?;
        std::fs::write(&path, content).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        if let Some(mode) = f.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode & 0o777));
        }
    }
    Ok(())
}

fn extract_tar<R: Read>(reader: R, target: &Path) -> Result<(), String> {
    let mut a = tar::Archive::new(reader);
    for entry in a.entries().map_err(|e| format!("tar : {e}"))? {
        let mut e = entry.map_err(|e| format!("tar : {e}"))?;
        let rel = e.path().map_err(|e| e.to_string())?.to_string_lossy().to_string();
        let Some(path) = safe_join(target, &rel) else { continue };
        match e.header().entry_type() {
            tar::EntryType::Directory => {
                std::fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            }
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                let mut content = Vec::new();
                e.read_to_end(&mut content).map_err(|e| e.to_string())?;
                std::fs::write(&path, content).map_err(|e| e.to_string())?;
                #[cfg(unix)]
                if let Ok(mode) = e.header().mode() {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode & 0o777));
                }
            }
            _ => {}
        }
    }
    Ok(())
}
