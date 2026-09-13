//! Dépôts synthétiques : construits dans un répertoire de travail éphémère
//! avec un commit déterministe (auteur et date fixes, contenu trié), puis
//! poussés en HTTPS avec les identifiants de l'API (ce que fait clever-tools).
//! Même contenu ⇒ même commit ⇒ push sauté (CLAUDE.md §7).

use std::path::Path as FsPath;

use git2::{Cred, IndexEntry, IndexTime, Oid, PushOptions, RemoteCallbacks, Repository, Signature, Time};

use crate::plan::sources::File;

const AUTHOR: &str = "clever-compose";
const EMAIL: &str = "clever-compose@localhost";

/// Construit le dépôt dans `dir` à partir des fichiers et renvoie l'id du
/// commit (déterministe).
pub fn build(dir: &FsPath, files: &[File]) -> Result<Oid, String> {
    let repo = Repository::init(dir).map_err(|e| format!("git init : {e}"))?;
    let mut index = repo.index().map_err(|e| e.to_string())?;
    let mut sorted: Vec<&File> = files.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    for f in sorted {
        let entry = IndexEntry {
            ctime: IndexTime::new(0, 0),
            mtime: IndexTime::new(0, 0),
            dev: 0,
            ino: 0,
            mode: if f.executable { 0o100755 } else { 0o100644 },
            uid: 0,
            gid: 0,
            file_size: f.content.len() as u32,
            id: Oid::zero(),
            flags: 0,
            flags_extended: 0,
            path: f.path.clone().into_bytes(),
        };
        index.add_frombuffer(&entry, &f.content).map_err(|e| format!("index {} : {e}", f.path))?;
    }
    let tree_id = index.write_tree().map_err(|e| e.to_string())?;
    let tree = repo.find_tree(tree_id).map_err(|e| e.to_string())?;
    let sig = Signature::new(AUTHOR, EMAIL, &Time::new(0, 0)).map_err(|e| e.to_string())?;
    let commit = repo.commit(Some("HEAD"), &sig, &sig, "clever-compose", &tree, &[]).map_err(|e| format!("commit : {e}"))?;
    Ok(commit)
}

/// Le commit courant de la branche distante `master` (celle que la
/// plateforme déploie), s'il existe.
pub fn remote_head(dir: &FsPath, url: &str, creds: &(String, String)) -> Result<Option<Oid>, String> {
    let repo = Repository::open(dir).map_err(|e| e.to_string())?;
    let mut remote = repo.remote_anonymous(url).map_err(|e| e.to_string())?;
    let mut cb = RemoteCallbacks::new();
    let (u, p) = creds.clone();
    cb.credentials(move |_, _, _| Cred::userpass_plaintext(&u, &p));
    remote.connect_auth(git2::Direction::Fetch, Some(cb), None).map_err(|e| format!("connexion à {url} : {e}"))?;
    let heads = remote.list().map_err(|e| e.to_string())?;
    Ok(heads.iter().find(|h| h.name() == "refs/heads/master").map(|h| h.oid()))
}

/// Pousse HEAD sur `refs/heads/master` (forcé : l'historique synthétique
/// n'a qu'un commit).
pub fn push(dir: &FsPath, url: &str, creds: &(String, String)) -> Result<(), String> {
    let repo = Repository::open(dir).map_err(|e| e.to_string())?;
    let mut remote = repo.remote_anonymous(url).map_err(|e| e.to_string())?;
    let mut cb = RemoteCallbacks::new();
    let (u, p) = creds.clone();
    cb.credentials(move |_, _, _| Cred::userpass_plaintext(&u, &p));
    let err = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let err2 = err.clone();
    cb.push_update_reference(move |_, status| {
        if let Some(s) = status {
            *err2.lock().unwrap() = Some(s.to_string());
        }
        Ok(())
    });
    let mut opts = PushOptions::new();
    opts.remote_callbacks(cb);
    remote.push(&["+HEAD:refs/heads/master"], Some(&mut opts)).map_err(|e| format!("push vers {url} : {e}"))?;
    let failure = err.lock().unwrap().clone();
    match failure {
        Some(e) => Err(format!("push refusé : {e}")),
        None => Ok(()),
    }
}
