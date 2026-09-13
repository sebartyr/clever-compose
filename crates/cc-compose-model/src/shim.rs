//! Configuration figée du shim, écrite dans l'image dérivée au moment du plan.
//!
//! Tout ce qui est ici est **intrinsèque à l'image** (commande, utilisateur,
//! port réel). Ce qui est topologique (table DNS, secrets) passe par les
//! variables d'environnement pour ne pas changer le hash de l'image à chaque
//! modification de topologie. Voir CLAUDE.md §5 et §7.

use serde::{Deserialize, Serialize};

/// Emplacement de la configuration dans l'image dérivée.
pub const CONFIG_PATH: &str = "/etc/cc-shim.json";

/// Emplacement du fichier d'identifiants écrit au build de l'image.
///
/// Format : `uid:gid:gid1,gid2,...` sur une ligne. En musl statique,
/// `getpwnam` n'est pas fiable, donc la résolution du nom d'utilisateur se
/// fait par une étape `RUN` du Dockerfile généré, avec la libc de l'image.
pub const IDS_PATH: &str = "/etc/cc-shim.ids";

/// Port sur lequel la plateforme attend l'application. Ne jamais le changer :
/// personnaliser `CC_DOCKER_EXPOSED_HTTP_PORT` fait basculer le conteneur hors
/// du mode `--net host` (CLAUDE.md §3).
pub const PLATFORM_HTTP_PORT: u16 = 8080;

/// Port d'entrée du trafic TCP d'une redirection (`CC_DOCKER_EXPOSED_TCP_PORT`,
/// même interdiction de le changer).
pub const PLATFORM_TCP_PORT: u16 = 4040;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShimConfig {
    /// Commande complète, entrypoint et cmd d'origine concaténés. Redéfinir
    /// `ENTRYPOINT` dans une image dérivée remet à vide le `CMD` hérité, donc
    /// la commande est lue sur le registre au plan et figée ici.
    pub argv: Vec<String>,

    /// Répertoire de travail de l'image d'origine, s'il en déclare un.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,

    /// Utilisateur déclaré par l'image d'origine (`USER`). `None` signifie que
    /// l'image tourne en root, le shim n'abandonne alors aucun privilège. Les
    /// identifiants numériques sont lus dans [`IDS_PATH`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,

    /// Ce que le shim fait du port plateforme, voir [`PlatformPort`].
    pub port: PlatformPort,

    /// Relais TCP brut depuis [`PLATFORM_TCP_PORT`] vers le port réel, pour
    /// une redirection TCP de la plateforme. Ouvert une fois le port réel
    /// joignable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp: Option<TcpRelay>,

    /// Relais UDP depuis [`PLATFORM_TCP_PORT`] (même port d'entrée que le
    /// TCP) vers le port réel, pour une redirection UDP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub udp: Option<UdpRelay>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UdpRelay {
    pub target_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TcpRelay {
    pub target_port: u16,
}

/// Comportement du shim sur [`PLATFORM_HTTP_PORT`].
///
/// Constaté en phase 0 : le déployeur valide un déploiement par un GET HTTP
/// sur 8080 et accepte tout code 200 à 500 ; une réponse non HTTP ou aucune
/// écoute font échouer le déploiement, y compris pour un worker. Chaque
/// conteneur doit donc répondre en HTTP sur 8080.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase", deny_unknown_fields)]
pub enum PlatformPort {
    /// L'image écoute elle-même en HTTP sur 8080 : le shim ne touche à rien.
    Image,
    /// Service HTTP sur un autre port : relais TCP depuis 8080, ouvert une
    /// fois le port réel joignable.
    Forward { target_port: u16 },
    /// Service non HTTP (joint sur son port réel dans le network group) ou
    /// worker sans port : répondeur `200 OK`. Avec `ready_port`, la réponse
    /// n'est servie qu'une fois ce port joignable, pour que le succès du
    /// déploiement reflète la disponibilité réelle.
    Health {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ready_port: Option<u16>,
    },
}

/// Identifiants numériques résolus au build de l'image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ids {
    pub uid: u32,
    pub gid: u32,
    pub groups: Vec<u32>,
}

impl Ids {
    /// Parse le contenu de [`IDS_PATH`].
    pub fn parse(s: &str) -> Result<Self, String> {
        let s = s.trim();
        let mut parts = s.splitn(3, ':');
        let uid = parts
            .next()
            .ok_or("uid manquant")?
            .parse()
            .map_err(|e| format!("uid invalide : {e}"))?;
        let gid = parts
            .next()
            .ok_or("gid manquant")?
            .parse()
            .map_err(|e| format!("gid invalide : {e}"))?;
        let groups = parts
            .next()
            .unwrap_or("")
            .split(',')
            .filter(|g| !g.is_empty())
            .map(|g| g.parse().map_err(|e| format!("groupe invalide `{g}` : {e}")))
            .collect::<Result<_, _>>()?;
        Ok(Ids { uid, gid, groups })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_parse() {
        assert_eq!(
            Ids::parse("999:999:999,70\n").unwrap(),
            Ids { uid: 999, gid: 999, groups: vec![999, 70] }
        );
        assert_eq!(
            Ids::parse("1000:1000:").unwrap(),
            Ids { uid: 1000, gid: 1000, groups: vec![] }
        );
        assert!(Ids::parse("abc:1").is_err());
    }
}
