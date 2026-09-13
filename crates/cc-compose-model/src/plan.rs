//! Plan intermédiaire : le contrat entre le parsing et les trois backends
//! (exécuteur, émetteur Terraform, émetteur clever-tools). CLAUDE.md §5.
//!
//! Principes :
//!
//! - **Références symboliques.** Aucun identifiant plateforme (`app_xxx`,
//!   `ng_yyy`) n'apparaît dans le plan. Une ressource en désigne une autre
//!   par son [`ResourceId`], et une valeur qui dépend de la plateforme (le
//!   FQDN d'un membre, un identifiant d'addon) est une [`Reference`] dans un
//!   [`Template`]. Chaque backend résout à sa manière : l'exécuteur après
//!   création, Terraform par interpolation HCL, le script par variables.
//! - **Intrinsèque contre topologique.** Ce qui vient de l'image
//!   ([`ImageIntrinsics`], [`ShimConfig`]) est figé dans le dépôt
//!   synthétique et ne change qu'avec l'image. Ce qui vient de la topologie
//!   (table DNS, identifiants, liaisons) passe par des variables
//!   d'environnement et des ressources de liaison, et ne provoque qu'un
//!   redémarrage (§7).
//! - **Ordre.** `resources` est dans l'ordre topologique de création ; les
//!   backends peuvent le suivre tel quel.
//!
//! Toute évolution de ce fichier se discute avant d'être codée (§13).

use serde::{Deserialize, Serialize};

use crate::shim::ShimConfig;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// Nom du projet Compose, préfixe des noms plateforme.
    pub project: String,
    /// Empreinte de la spécification traduite (arbre canonique), posée en
    /// tag sur chaque ressource pour la réconciliation sans état (§7).
    pub spec_hash: String,
    pub resources: Vec<Resource>,
}

/// Identifiant symbolique, stable, dérivé des noms Compose :
/// `app/web`, `addon/db`, `ng/default`, `member/web@default`,
/// `domain/web/0`, `tcp/web`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ResourceId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "resource", rename_all = "snake_case")]
pub enum Resource {
    App(App),
    Addon(Addon),
    NetworkGroup(NetworkGroup),
    Membership(Membership),
    Redirection(Redirection),
    Domain(Domain),
}

impl Resource {
    pub fn id(&self) -> &ResourceId {
        match self {
            Resource::App(r) => &r.id,
            Resource::Addon(r) => &r.id,
            Resource::NetworkGroup(r) => &r.id,
            Resource::Membership(r) => &r.id,
            Resource::Redirection(r) => &r.id,
            Resource::Domain(r) => &r.id,
        }
    }
}

/// Application Docker Clever Cloud, une par service Compose. Une tâche
/// (service one-shot) est une application avec [`AppKind::Task`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct App {
    pub id: ResourceId,
    /// Nom du service Compose.
    pub service: String,
    /// Nom plateforme : `<projet>-<service>`.
    pub name: String,
    pub kind: AppKind,
    pub region: String,
    pub scaling: Scaling,
    /// D'où vient le code poussé : dépôt synthétique autour d'une image, ou
    /// contexte de build de l'utilisateur.
    pub source: Source,
    /// Ce que l'image déclare, lu sur le registre au plan.
    pub image: ImageIntrinsics,
    /// Configuration du shim figée dans l'image dérivée.
    pub shim: ShimConfig,
    /// Variables d'environnement de l'application, après réécriture des
    /// noms de services et câblage des addons. Triées par clé.
    pub env: Vec<EnvVar>,
    /// Applications et addons à créer et déployer avant celle-ci
    /// (`depends_on`, ordre topologique).
    pub depends_on: Vec<ResourceId>,
    /// `CC_HEALTH_CHECK_PATH`, si `healthcheck` est HTTP.
    pub health_check_path: Option<String>,
    /// Secrets Compose montés dans ce service. La valeur vient de `source`
    /// et alimente la variable [`Reference::Secret`] de `env` ; le shim
    /// écrit le fichier (`shim.secrets`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<SecretBinding>,
    /// Tags plateforme (`compose-project`, `compose-service`, empreinte).
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppKind {
    Service,
    /// Clever Task : le conteneur exécute sa commande puis s'arrête.
    Task,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scaling {
    pub flavor: String,
    pub min_instances: u32,
    pub max_instances: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Source {
    /// Dépôt construit en mémoire : `FROM <image>`, le shim, sa config, les
    /// fichiers copiés (bind mounts de fichiers, §6).
    Synthetic {
        image: String,
        /// Registre privé : identifiants via `CC_DOCKER_LOGIN_*`, jamais
        /// dans le plan.
        private_registry: bool,
        files: Vec<CopiedFile>,
    },
    /// Contexte de build de l'utilisateur, poussé tel quel avec le shim
    /// ajouté par une étape finale.
    Context {
        /// Chemin du contexte, relatif au répertoire du projet (`./web`),
        /// pour que le plan soit le même sur tout poste.
        context: String,
        /// Dockerfile relatif au contexte.
        dockerfile: String,
        files: Vec<CopiedFile>,
    },
}

/// Fichier de l'utilisateur figé dans l'image (bind mount d'un fichier).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopiedFile {
    /// Chemin sur le poste, relatif au répertoire du projet.
    pub source: String,
    /// Chemin dans le conteneur.
    pub target: String,
    pub read_only: bool,
}

/// Ce que la configuration de l'image déclare (lu sur le registre, §8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageIntrinsics {
    pub entrypoint: Vec<String>,
    pub cmd: Vec<String>,
    pub user: Option<String>,
    pub workdir: Option<String>,
    /// Ports `EXPOSE` de l'image.
    pub exposed_ports: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVar {
    pub key: String,
    pub value: Template,
}

/// Valeur composée de littéraux et de références résolues par le backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Template(pub Vec<Segment>);

impl Template {
    pub fn literal(s: impl Into<String>) -> Self {
        Template(vec![Segment::Literal(s.into())])
    }

    /// La valeur si elle ne contient aucune référence.
    pub fn as_literal(&self) -> Option<String> {
        let mut out = String::new();
        for seg in &self.0 {
            match seg {
                Segment::Literal(s) => out.push_str(s),
                Segment::Ref(_) => return None,
            }
        }
        Some(out)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Segment {
    Literal(String),
    Ref(Reference),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "ref", rename_all = "snake_case")]
pub enum Reference {
    /// FQDN d'un membre dans son network group
    /// (`<memberId>.m.<ngId>.cc-ng.cloud`), connu dès la création de l'app
    /// et du groupe (phase 0).
    MemberFqdn { member: ResourceId },
    /// Variable injectée par la liaison d'un addon (`POSTGRESQL_ADDON_HOST`
    /// ...). Expansée au démarrage par le shim, jamais figée (§6.2).
    AddonEnv { addon: ResourceId, var: String },
    /// Valeur d'un secret Compose (`App.secrets`), lue par le backend depuis
    /// sa source au moment de poser l'environnement : jamais dans le plan,
    /// ni en littéral dans un export (§10).
    Secret { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretBinding {
    /// Nom du secret de premier niveau.
    pub name: String,
    pub source: SecretSource,
    /// Variable de l'application qui porte la valeur.
    pub env: String,
    /// Fichier dans le conteneur.
    pub target: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gid: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SecretSource {
    /// Fichier sur le poste, relatif au répertoire du projet.
    File { path: String },
    /// Variable de l'environnement du poste au moment du `up`.
    Environment { var: String },
}

/// Addon managé substitué à un service stateful (§6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Addon {
    pub id: ResourceId,
    /// Service Compose remplacé.
    pub service: String,
    pub name: String,
    /// `postgresql-addon`, `mysql-addon`, `redis-addon`, `mongodb-addon`,
    /// `es-addon`, `cellar-addon`, `config-provider`.
    pub provider: String,
    pub plan: String,
    pub region: String,
    /// Version demandée par le tag de l'image, si le fournisseur la porte.
    pub version: Option<String>,
    pub tags: Vec<String>,
}

/// Un network group par réseau Compose (§7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkGroup {
    pub id: ResourceId,
    /// Réseau Compose (`default`, `frontend`...).
    pub network: String,
    /// Label plateforme : `<projet>-<réseau>`.
    pub name: String,
    pub tags: Vec<String>,
}

/// Appartenance d'une application ou d'un addon à un groupe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Membership {
    pub id: ResourceId,
    pub member: ResourceId,
    pub group: ResourceId,
    /// Noms par lesquels les autres membres désignent celui-ci (nom du
    /// service et `aliases` du réseau). C'est la clé de la table DNS.
    pub aliases: Vec<String>,
}

/// Redirection TCP ou UDP : la plateforme attribue un port externe (> 5000)
/// sur un namespace de reverse proxies ; le trafic TCP entre dans le
/// conteneur par 4040, relayé par le shim vers `target_port`. Le port
/// attribué est une sortie de la création, jamais une entrée.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Redirection {
    pub id: ResourceId,
    pub app: ResourceId,
    /// `default` (domaine propre) ou `cleverapps` (domaine cleverapps.io).
    pub namespace: String,
    pub protocol: Protocol,
    /// Port Compose publié (`ports:`), pour la lisibilité.
    pub target_port: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Tcp,
    Udp,
}

/// Domaine public d'une application.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Domain {
    pub id: ResourceId,
    pub app: ResourceId,
    pub fqdn: String,
}
