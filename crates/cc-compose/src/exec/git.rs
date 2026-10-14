//! Dépôts synthétiques : construits dans un répertoire de travail éphémère,
//! puis poussés en HTTPS avec les identifiants de l'API (ce que fait
//! clever-tools).
//!
//! **L'historique du dépôt distant est repris** (décision du 14/10/2026) :
//! le commit du `up` a pour parent le `master` distant, récupéré avant d'être
//! construit, et le push est en avance rapide. Le dépôt est l'un des formats
//! de sortie de l'outil (comme l'export Terraform, qui déploie depuis le
//! dépôt de l'utilisateur) : son historique dit ce qui a été déployé et
//! quand, et un `up` ne peut plus effacer ce qu'un autre a poussé.
//!
//! Ce qui remplace la détermination du commit : l'**arbre** décide. Le
//! contenu est déterministe (entrées triées, modes figés, blobs identiques) ;
//! si le commit distant porte déjà cet arbre, il n'y a rien à pousser
//! (`Built::Unchanged`, pas de commit vide), ce qui garde `up` idempotent
//! (CLAUDE.md §7). La date du commit, elle, est réelle : un historique
//! empilé daté de 1970 ne se lit pas.

use std::path::Path as FsPath;

use git2::{Cred, IndexEntry, IndexTime, Oid, PushOptions, RemoteCallbacks, Repository, Signature, Time};

use crate::plan::sources::File;

const AUTHOR: &str = "clever-compose";
const EMAIL: &str = "clever-compose@localhost";
/// Refspec de récupération : toutes les branches du distant, pour que le
/// dépôt vide (jamais déployé) ne soit pas une erreur.
const FETCH_REFSPEC: &str = "+refs/heads/*:refs/remotes/origin/*";
/// La branche que la plateforme déploie.
const REMOTE_BRANCH: &str = "refs/remotes/origin/master";

/// Ce que `build` a fait du contenu demandé.
pub enum Built {
    /// Le commit en place porte déjà cet arbre : rien à pousser.
    Unchanged(Oid),
    /// Nouveau commit (sur le parent s'il y en avait un) à pousser.
    Commit(Oid),
}

impl Built {
    pub fn oid(&self) -> Oid {
        match self {
            Built::Unchanged(o) | Built::Commit(o) => *o,
        }
    }
}

fn callbacks(creds: &(String, String)) -> RemoteCallbacks<'static> {
    let mut cb = RemoteCallbacks::new();
    let (u, p) = creds.clone();
    cb.credentials(move |_, _, _| Cred::userpass_plaintext(&u, &p));
    cb
}

/// Signature à l'instant présent (l'historique est empilé : une date fixe
/// ne dirait rien).
fn signature() -> Result<Signature<'static>, String> {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    Signature::new(AUTHOR, EMAIL, &Time::new(secs as i64, 0)).map_err(|e| e.to_string())
}

/// Récupère l'historique distant dans `dir` (dépôt initialisé par
/// [`build`] ou à initialiser ici) et rend le commit de `master`, s'il y en
/// a un. Les objets récupérés restent dans `dir` : le commit suivant peut
/// s'y rattacher.
pub fn fetch_head(dir: &FsPath, url: &str, creds: &(String, String)) -> Result<Option<Oid>, String> {
    let repo = Repository::init(dir).map_err(|e| format!("git init : {e}"))?;
    let mut remote = repo.remote_anonymous(url).map_err(|e| e.to_string())?;
    let mut opts = git2::FetchOptions::new();
    opts.remote_callbacks(callbacks(creds));
    remote.fetch(&[FETCH_REFSPEC], Some(&mut opts), None).map_err(|e| format!("récupération depuis {url} : {e}"))?;
    Ok(repo.find_reference(REMOTE_BRANCH).ok().and_then(|r| r.target()))
}

/// Construit l'arbre des fichiers dans `dir` et le commit sur `parent`
/// (le `master` distant rendu par [`fetch_head`], s'il existe). Rend
/// [`Built::Unchanged`] si `parent` porte déjà exactement cet arbre.
pub fn build(dir: &FsPath, files: &[File], parent: Option<Oid>) -> Result<Built, String> {
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
    let parent = match parent {
        Some(oid) => Some(repo.find_commit(oid).map_err(|e| format!("commit distant {oid} : {e}"))?),
        None => None,
    };
    // Même arbre que le commit en place : pas de commit vide, pas de push.
    if let Some(p) = &parent {
        if p.tree_id() == tree_id {
            return Ok(Built::Unchanged(p.id()));
        }
    }
    let tree = repo.find_tree(tree_id).map_err(|e| e.to_string())?;
    let sig = signature()?;
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    let commit = repo.commit(Some("HEAD"), &sig, &sig, "déploiement clever-compose", &tree, &parents).map_err(|e| format!("commit : {e}"))?;
    Ok(Built::Commit(commit))
}

/// Pousse HEAD sur `refs/heads/master`. En avance rapide (le commit
/// descend du `master` distant récupéré juste avant) : un refus signale
/// que le distant a changé entre-temps, et n'écrase rien. `replace` ne
/// sert qu'au repli, quand l'historique distant n'a pas pu être lu.
pub fn push(dir: &FsPath, url: &str, creds: &(String, String), replace: bool) -> Result<(), String> {
    let repo = Repository::open(dir).map_err(|e| e.to_string())?;
    let mut remote = repo.remote_anonymous(url).map_err(|e| e.to_string())?;
    let mut cb = callbacks(creds);
    let err = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    let err2 = err.clone();
    cb.push_update_reference(move |_, status| {
        if let Some(s) = status {
            *err2.lock().unwrap_or_else(|e| e.into_inner()) = Some(s.to_string());
        }
        Ok(())
    });
    let mut opts = PushOptions::new();
    opts.remote_callbacks(cb);
    let refspec = if replace { "+HEAD:refs/heads/master" } else { "HEAD:refs/heads/master" };
    remote.push(&[refspec], Some(&mut opts)).map_err(|e| format!("push vers {url} : {e}"))?;
    let failure = err.lock().unwrap_or_else(|e| e.into_inner()).clone();
    match failure {
        Some(e) => Err(format!("push refusé : {e} (le dépôt de l'application a changé pendant le `up` ? relancez)")),
        None => Ok(()),
    }
}

/// Écrit le dépôt synthétique comme branche `refs/heads/clever-compose/<service>`
/// dans le dépôt git qui contient `project_dir` (objets écrits dans son
/// ODB, aucun identifiant nécessaire). La branche est **prolongée** si elle
/// existe : c'est elle que l'export Terraform déploie
/// (`deployment.commit`), son historique est celui des déploiements. Rend
/// l'id du commit, s'il est nouveau, et l'URL de `origin` si elle existe.
pub fn write_branch(project_dir: &FsPath, service: &str, files: &[File]) -> Result<(Built, Option<String>), String> {
    let repo = Repository::discover(project_dir).map_err(|_| format!("{} n'est pas dans un dépôt git : les branches clever-compose/* n'ont nulle part où aller", project_dir.display()))?;
    let refname = format!("refs/heads/clever-compose/{service}");
    let tree_id = build_tree(&repo, files)?;
    let parent = repo.find_reference(&refname).ok().and_then(|r| r.target()).and_then(|oid| repo.find_commit(oid).ok());
    let origin = repo.find_remote("origin").ok().and_then(|r| r.url().map(str::to_string));
    if let Some(p) = &parent {
        if p.tree_id() == tree_id {
            return Ok((Built::Unchanged(p.id()), origin));
        }
    }
    let tree = repo.find_tree(tree_id).map_err(|e| e.to_string())?;
    let sig = signature()?;
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    let commit = repo.commit(None, &sig, &sig, &format!("déploiement clever-compose : {service}"), &tree, &parents).map_err(|e| format!("branche {refname} : {e}"))?;
    repo.reference(&refname, commit, true, "clever-compose export").map_err(|e| format!("branche {refname} : {e}"))?;
    Ok((Built::Commit(commit), origin))
}

/// Arbre git des fichiers, blobs écrits dans l'ODB du dépôt, répertoires
/// imbriqués construits récursivement, entrées triées.
fn build_tree(repo: &Repository, files: &[File]) -> Result<Oid, String> {
    #[derive(Default)]
    struct Dir {
        files: Vec<(String, Oid, bool)>,
        dirs: std::collections::BTreeMap<String, Dir>,
    }
    let mut root = Dir::default();
    for f in files {
        let oid = repo.blob(&f.content).map_err(|e| format!("blob {} : {e}", f.path))?;
        let mut parts: Vec<&str> = f.path.split('/').collect();
        let name = parts.pop().unwrap().to_string();
        let mut cur = &mut root;
        for p in parts {
            cur = cur.dirs.entry(p.to_string()).or_default();
        }
        cur.files.push((name, oid, f.executable));
    }
    fn write(repo: &Repository, d: &Dir) -> Result<Oid, String> {
        let mut b = repo.treebuilder(None).map_err(|e| e.to_string())?;
        for (name, sub) in &d.dirs {
            let oid = write(repo, sub)?;
            b.insert(name, oid, 0o040000).map_err(|e| e.to_string())?;
        }
        for (name, oid, exec) in &d.files {
            b.insert(name, *oid, if *exec { 0o100755 } else { 0o100644 }).map_err(|e| e.to_string())?;
        }
        b.write().map_err(|e| e.to_string())
    }
    write(repo, &root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exec::reconcile::fresh_dir;

    fn file(path: &str, content: &str) -> File {
        File { path: path.into(), content: content.as_bytes().to_vec(), executable: false }
    }

    /// Un dépôt nu local sert de « plateforme » : le transport local de
    /// libgit2 passe par les mêmes chemins que HTTPS (fetch, push, refus
    /// de non-avance-rapide), sans réseau ni identifiants.
    fn bare() -> (std::path::PathBuf, String) {
        let dir = fresh_dir(&std::env::temp_dir(), "cc-compose-test-", "bare").unwrap();
        Repository::init_bare(&dir).unwrap();
        let url = dir.display().to_string();
        (dir, url)
    }

    fn work(what: &str) -> std::path::PathBuf {
        fresh_dir(&std::env::temp_dir(), "cc-compose-test-", what).unwrap()
    }

    /// Le commit de `refs/heads/master` du dépôt nu : son arbre et ses parents.
    fn head_of(bare: &std::path::Path) -> (Oid, Vec<Oid>) {
        let repo = Repository::open(bare).unwrap();
        let c = repo.find_reference("refs/heads/master").unwrap().peel_to_commit().unwrap();
        (c.id(), c.parent_ids().collect())
    }

    #[test]
    fn history_is_stacked_not_replaced() {
        let creds = (String::new(), String::new());
        let (bare_dir, url) = bare();
        let a = vec![file("Dockerfile", "FROM alpine\n")];
        let b = vec![file("Dockerfile", "FROM alpine:3.20\n")];

        // Dépôt distant vide : aucun parent, commit racine.
        let w1 = work("w1");
        assert_eq!(fetch_head(&w1, &url, &creds).unwrap(), None);
        let c1 = match build(&w1, &a, None).unwrap() {
            Built::Commit(oid) => oid,
            Built::Unchanged(_) => panic!("dépôt vide : un commit est attendu"),
        };
        push(&w1, &url, &creds, false).unwrap();
        assert_eq!(head_of(&bare_dir), (c1, vec![]));

        // Même contenu : rien à pousser, et le commit distant est conservé.
        let w2 = work("w2");
        let parent = fetch_head(&w2, &url, &creds).unwrap();
        assert_eq!(parent, Some(c1));
        match build(&w2, &a, parent).unwrap() {
            Built::Unchanged(oid) => assert_eq!(oid, c1),
            Built::Commit(_) => panic!("arbre identique : aucun commit ne doit être créé"),
        }

        // Contenu changé : le commit s'empile sur le distant.
        let w3 = work("w3");
        let parent = fetch_head(&w3, &url, &creds).unwrap();
        let c2 = match build(&w3, &b, parent).unwrap() {
            Built::Commit(oid) => oid,
            Built::Unchanged(_) => panic!("contenu changé : un commit est attendu"),
        };
        push(&w3, &url, &creds, false).unwrap();
        assert_eq!(head_of(&bare_dir), (c2, vec![c1]), "l'historique compte deux commits");

        let _ = std::fs::remove_dir_all(&bare_dir);
        for w in [w1, w2, w3] {
            let _ = std::fs::remove_dir_all(w);
        }
    }

    #[test]
    fn diverged_remote_is_refused_then_replaceable() {
        let creds = (String::new(), String::new());
        let (bare_dir, url) = bare();
        let a = vec![file("Dockerfile", "FROM alpine\n")];
        let b = vec![file("Dockerfile", "FROM alpine:3.20\n")];
        let c = vec![file("Dockerfile", "FROM debian\n")];

        let w1 = work("d1");
        fetch_head(&w1, &url, &creds).unwrap();
        build(&w1, &a, None).unwrap();
        push(&w1, &url, &creds, false).unwrap();

        // Deux `up` partis du même état : le second à pousser est refusé
        // plutôt que d'écraser le premier.
        let (slow, fast) = (work("d-slow"), work("d-fast"));
        let parent = fetch_head(&slow, &url, &creds).unwrap();
        build(&slow, &b, parent).unwrap();
        let parent_fast = fetch_head(&fast, &url, &creds).unwrap();
        build(&fast, &c, parent_fast).unwrap();
        push(&fast, &url, &creds, false).unwrap();
        let (head_after_fast, _) = head_of(&bare_dir);

        let refused = push(&slow, &url, &creds, false).unwrap_err();
        assert!(refused.contains("push"), "{refused}");
        assert_eq!(head_of(&bare_dir).0, head_after_fast, "le distant n'a pas bougé");
        // Le repli (historique distant illisible) écrase, lui.
        push(&slow, &url, &creds, true).unwrap();
        assert_ne!(head_of(&bare_dir).0, head_after_fast);

        let _ = std::fs::remove_dir_all(&bare_dir);
        for w in [w1, slow, fast] {
            let _ = std::fs::remove_dir_all(w);
        }
    }
}
