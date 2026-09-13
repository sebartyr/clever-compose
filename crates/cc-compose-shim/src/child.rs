//! Lancement et supervision de l'enfant, sémantique de référence : tini.
//!
//! - abandon des privilèges dans l'enfant, avec `initgroups` équivalent
//!   (`setgroups` sur la liste résolue au build) ;
//! - relais des signaux de terminaison à l'enfant ;
//! - récupération de tous les zombies, pas seulement l'enfant direct ;
//! - code de sortie : celui de l'enfant, ou 128 + signal.

use std::os::unix::process::CommandExt;
use std::process::Command;

use cc_compose_model::shim::{Ids, ShimConfig, IDS_PATH};
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::oneshot;

pub struct Child {
    pid: libc::pid_t,
    exited: oneshot::Receiver<i32>,
}

pub fn spawn(config: &ShimConfig, env_remove: &[String]) -> Result<Child, String> {
    let mut cmd = Command::new(&config.argv[0]);
    cmd.args(&config.argv[1..]);
    for var in env_remove {
        cmd.env_remove(var);
    }
    if let Some(dir) = &config.workdir {
        cmd.current_dir(dir);
    }
    if let Some(user) = &config.user {
        let raw = std::fs::read_to_string(IDS_PATH)
            .map_err(|e| format!("utilisateur `{user}` : lecture de {IDS_PATH} : {e}"))?;
        let ids = Ids::parse(&raw).map_err(|e| format!("{IDS_PATH} : {e}"))?;
        // Abandon des privilèges dans l'enfant, après le fork, dans l'ordre
        // setgroups, setgid, setuid (`CommandExt::groups` est instable ;
        // `Command::uid/gid` s'appliqueraient avant `pre_exec`, trop tôt pour
        // setgroups). Équivalent d'initgroups avec la liste résolue au build :
        // un uid/gid seul casse les images qui dépendent d'un groupe
        // secondaire (CLAUDE.md §8).
        let groups: Vec<libc::gid_t> = ids.groups.iter().map(|g| *g as libc::gid_t).collect();
        let (uid, gid) = (ids.uid as libc::uid_t, ids.gid as libc::gid_t);
        // SAFETY: appels async-signal-safe, sans allocation, dans l'enfant.
        unsafe {
            cmd.pre_exec(move || {
                if libc::setgroups(groups.len() as _, groups.as_ptr()) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::setgid(gid) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::setuid(uid) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    let child = cmd
        .spawn()
        .map_err(|e| format!("exec de `{}` : {e}", config.argv[0]))?;
    let pid = child.id() as libc::pid_t;
    // On ne se sert plus du handle std : le reaping se fait par waitpid(-1)
    // pour récupérer aussi les petits-enfants orphelins. Oublier le handle
    // évite un double wait.
    std::mem::forget(child);

    let (tx, rx) = oneshot::channel();
    std::thread::Builder::new()
        .name("reaper".into())
        .spawn(move || reap(pid, tx))
        .map_err(|e| format!("thread reaper : {e}"))?;

    Ok(Child { pid, exited: rx })
}

/// Boucle bloquante : récupère tous les zombies, publie le statut de l'enfant
/// direct quand il sort.
fn reap(child: libc::pid_t, tx: oneshot::Sender<i32>) {
    let mut tx = Some(tx);
    loop {
        let mut status: libc::c_int = 0;
        // SAFETY: appel direct de waitpid avec un pointeur valide.
        let pid = unsafe { libc::waitpid(-1, &mut status, 0) };
        if pid == child {
            if let Some(tx) = tx.take() {
                let _ = tx.send(status);
            }
            // On continue tant qu'il reste des descendants à récupérer.
        } else if pid < 0 {
            let err = std::io::Error::last_os_error();
            match err.raw_os_error() {
                Some(libc::EINTR) => continue,
                // Plus aucun enfant : fin de la supervision.
                Some(libc::ECHILD) => return,
                _ => {
                    eprintln!("cc-shim: waitpid : {err}");
                    return;
                }
            }
        }
    }
}

impl Child {
    pub async fn supervise(self) -> Result<u8, String> {
        let forwarded = [
            SignalKind::terminate(),
            SignalKind::interrupt(),
            SignalKind::hangup(),
            SignalKind::quit(),
            SignalKind::user_defined1(),
            SignalKind::user_defined2(),
        ];
        let mut streams = Vec::with_capacity(forwarded.len());
        for kind in forwarded {
            let s = signal(kind).map_err(|e| format!("signal {:?} : {e}", kind))?;
            streams.push((kind, s));
        }

        let pid = self.pid;
        let mut exited = self.exited;
        loop {
            // Un select dynamique sur N flux de signaux : on attend le premier
            // prêt via une petite fonction d'agrégation.
            let sig = next_signal(&mut streams);
            tokio::select! {
                status = &mut exited => {
                    let status = status.map_err(|_| "reaper terminé sans statut".to_string())?;
                    return Ok(exit_code(status));
                }
                kind = sig => {
                    // SAFETY: kill sur un pid connu, encore non récupéré tant
                    // que `exited` n'a pas répondu.
                    unsafe { libc::kill(pid, kind.as_raw_value()) };
                }
            }
        }
    }
}

async fn next_signal(streams: &mut [(SignalKind, tokio::signal::unix::Signal)]) -> SignalKind {
    use std::future::poll_fn;
    use std::task::Poll;
    poll_fn(|cx| {
        for (kind, s) in streams.iter_mut() {
            if s.poll_recv(cx).is_ready() {
                return Poll::Ready(*kind);
            }
        }
        Poll::Pending
    })
    .await
}

fn exit_code(status: libc::c_int) -> u8 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status) as u8
    } else if libc::WIFSIGNALED(status) {
        (128 + libc::WTERMSIG(status)) as u8
    } else {
        125
    }
}
