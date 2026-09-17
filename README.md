# clever-compose

Fait tourner un `docker compose` sur Clever Cloud : chaque service devient une
application Docker, chaque réseau un network group, les bases reconnues des
addons managés. Deux promesses : **transparence** (`clever-compose up` sans
réécrire le compose) et **sortie réversible** (export Terraform et script
clever-tools). Cadrage complet dans `CLAUDE.md`, décisions et état dans
`summary.md`.

## Installation

```sh
rustup target add x86_64-unknown-linux-musl   # une fois : cible du shim
cargo build --release                                            # CLI + API + interface
cargo build --release --no-default-features --features api      # CLI + API (interface servie ailleurs)
cargo build --release --no-default-features                     # CLI seule
# binaire : target/release/clever-compose, autonome
```

Trois binaires selon les features : `ui` (défaut, commandes `ui` et
`api`), `api` (commande `api` seulement, pas d'interface embarquée), ou
rien (CLI seule, qui peut tout de même s'exécuter à distance contre un
serveur `api`, voir plus bas). `ui-bundled` empaquette le front Vite.

Le shim (PID 1 des conteneurs, crate `cc-compose-shim`) est compilé pour
musl par le `build.rs` de la CLI (dans `target/shim-build/`, relancé quand
ses sources changent) et embarqué dans le binaire. Sans la cible musl, le
build passe avec un avertissement et la CLI demande `CLEVER_COMPOSE_SHIM_PATH` à
l'usage ; `CLEVER_COMPOSE_SHIM_BIN` au build embarque un shim déjà compilé.

## Usage

```sh
clever-compose login                     # danse OAuth 1.0 dans le navigateur (consumer clever-compose, callback local
                                         # 127.0.0.1:8080) ; jetons gardés dans ~/.config/clever-cloud/clever-compose.json
                                         # (ou CLEVER_TOKEN/CLEVER_SECRET, CLEVER_COMPOSE_API_TOKEN ; CLEVER_CONSUMER_KEY/SECRET pour un autre consumer)
export CLEVER_COMPOSE_ORG=orga_xxx                   # ou --org

clever-compose plan   [--json]                    # ce qui serait créé, coût, diagnostics ; aucun appel à l'API
clever-compose up     [--dry-run] [--yes] [--no-wait] [--allow-ephemeral] [--assets]
clever-compose stop                               # arrête les applications, ne supprime rien (comme docker compose stop)
clever-compose start                              # relance ce qui est arrêté ; `up` le fait aussi
clever-compose down   [--all] [--dry-run] [--yes]  # supprime ; par nom de projet et tags, sans relire le compose
clever-compose ps
clever-compose logs   [SERVICE...] [--since 1h] [-n N] [--follow]
clever-compose seed   --service db --dump dump.sql [--local]
clever-compose export [all|terraform|script] -o DIR [--branches]
clever-compose config --format json               # l'équivalent de `docker compose config`
clever-compose orgs                               # organisations accessibles avec les identifiants
clever-compose ui     [--listen 127.0.0.1:8080] [--no-open] [--public-url URL]   # interface web locale
clever-compose api    [--listen 127.0.0.1:8080] [--cors ORIGINE...] [--public-url URL] [--api-key CLÉ]  # API seule, interface déployée à part
```

Options de projet, avant ou après la commande, comme `docker compose` :
`-f FICHIER` (répétable), `-p NOM`, `--project-directory DIR`, `--env-file`,
`--profile`. `--help` sur chaque commande.

- `plan` est un dry-run pur : il lit les images sur leur registre (cache
  local), produit le plan intermédiaire en JSON et tous les diagnostics, avec
  le chemin et la position dans le fichier source. Chaque clé Compose est
  traduite, ignorée avec avertissement, ou refusée : rien n'est approximé.
- `up` réconcilie sans état local : les ressources se retrouvent par nom et
  par tags, l'environnement n'est remplacé que s'il change, les dépôts
  synthétiques ont un commit déterministe et le push est sauté s'il est
  déjà là. Les addons sont facturés : `--yes` confirme leur création.
- `--allow-ephemeral` accepte les volumes des services non substituables
  comme stockage éphémère ; `x-clever.ephemeral: true` garde une base en
  conteneur au lieu d'un addon.
- `seed` charge un dump local dans un addon (`.sql`/`.sql.gz`/`.sh` pour
  PostgreSQL et MySQL, `.dump` de `pg_dump -Fc` via `pg_restore`, `.js`/`.archive`/`.sh` pour MongoDB et DocumentDB,
  `.redis`/`.resp` pour Redis, `.ndjson` pour Elasticsearch), une fois par
  contenu (témoin `cc_compose_init`) ; par une Clever Task, ou depuis le
  poste avec `--local` (Docker s'il répond, sinon le client installé).
- `down`, `ps` et `logs` ne relisent pas le compose : le nom du projet et
  les tags posés par `up` suffisent. `logs` lit les applications et les
  addons, `--since 1h` ou un horodatage RFC 3339, `-f` pour suivre.
- `export terraform --branches` écrit les branches `clever-compose/<service>`
  dans le dépôt git du projet, que `deployment.repository` référence : une
  URL distante à pousser (`git push origin 'refs/heads/clever-compose/*'`),
  ou `file:///chemin/du/dépôt` (le provider l'ouvre sur place). Ce que le
  provider ne sait pas exprimer sort en avertissement : Clever Task
  (`terraform-task`), options d'addon comme DocumentDB (ressource omise,
  id réel attendu dans la variable `addon_<service>_real_id`), UDP.
- `--assets` (ou `x-clever.assets: true` au niveau projet) conserve les
  fichiers du projet (compose, fichiers montés, scripts) et le plan dans un
  bucket Cellar que vous créez vous-même, configuré par `CLEVER_COMPOSE_ASSETS_BUCKET`
  et les variables de l'addon (`CELLAR_ADDON_HOST`, `CELLAR_ADDON_KEY_ID`,
  `CELLAR_ADDON_KEY_SECRET`) ; un seul bucket, un préfixe `<projet>/` par
  projet. L'interface lit alors un projet déployé sans son dossier et peut
  le rapatrier en projet local ; `down` retire les objets du projet, jamais
  le bucket. L'outil ne crée ni addon ni bucket.
- Image sur un registre privé : `docker login <hôte>` sur le poste suffit,
  `up` pose `CC_DOCKER_LOGIN_*` depuis la configuration Docker et l'export
  Terraform attend `registry_<hôte>_user`/`_password` en variables.

## Interface web

`clever-compose ui` lance un serveur local (adresse et port par `--listen`)
et ouvre le navigateur. Tout ce que la CLI sait faire y est : projets
locaux (le répertoire courant, un répertoire du poste, ou une archive
`.zip`/`.tar.gz` extraite dans un répertoire temporaire), diagramme des
ressources (network groups, applications, tâches, addons, dépendances) avec
le détail au clic, `plan` et diagnostics, éditeur du compose (enregistrer
replanifie), `up`/`seed`/`down` avec le journal en direct, `logs`, exports
Terraform et script. Les projets déployés de l'organisation s'y retrouvent
avec l'état des instances.

Identifiants : **chaque client envoie les siens**, à chaque requête
(`X-CC-Token`/`X-CC-Secret`, ou `X-CC-Api-Token`), et le
serveur les vérifie chez Clever Cloud (`/v2/self`, cache de cinq minutes)
avant d'agir en son nom ; le serveur ne garde aucune session. Le consumer
OAuth est celui de clever-compose, embarqué ; `CLEVER_CONSUMER_KEY` /
`CLEVER_CONSUMER_SECRET` dans l'environnement le remplacent toujours.
Clever Cloud n'accepte le callback (`<base>/api/session/oauth/callback`)
que sous la **base URL déclarée pour le consumer** dans la console : elle
doit être l'adresse publique du serveur (`http://127.0.0.1:8080/` sur le
poste, `https://app-….cleverapps.io/` pour une application Clever Cloud).
Cette adresse est déduite de chaque requête (`Host`, `X-Forwarded-Proto`
du proxy) ; `--public-url` la fixe si la déduction ne convient pas. Un
navigateur sans jetons voit un écran de connexion qui démarre tout seul :
la page part vers l'autorisation Clever Cloud et le callback ramène les
jetons dans le fragment de l'URL.

Sur une application Clever Cloud : `clever-compose ui --listen 0.0.0.0:8080
--api-key …` (ou `CLEVER_COMPOSE_LISTEN=0.0.0.0:8080` ; le port par défaut est 8080, celui
qu'attend la plateforme) (variables `CLEVER_COMPOSE_API_KEY`, et `CLEVER_CONSUMER_*` si le consumer
embarqué n'a pas cette base URL) ; rien d'autre à configurer.

Les jetons restent dans l'onglet (sessionStorage) ; « Se déconnecter »
les oublie. Des jetons refusés (expirés, révoqués) ramènent à l'écran de
connexion avec la raison. Pour l'usage local, un serveur lancé avec des
identifiants (`login` fait, ou variables d'environnement) les prête aux
clients qui n'en envoient pas : `clever-compose ui` sur son poste ne
demande rien. L'organisation se choisit dans la liste (retenue dans le
navigateur, envoyée en `X-CC-Org`). L'interface s'adapte aux
petits écrans (menu des projets en tiroir, détail sous le diagramme).

Scalabilité : le détail d'une application du diagramme porte un formulaire
(flavor minimale et maximale, instances minimales et maximales) ;
« Appliquer au compose » écrit les clés `x-clever` correspondantes dans le
fichier compose du projet sans toucher au reste (commentaires et ordre
conservés) et replanifie. L'onglet Compose offre « Télécharger » (le
fichier tel qu'affiché, pour remplacer celui de votre dépôt) et, quand un
bucket Cellar est configuré, « Mettre à jour les assets » (pousse compose,
`.env`, contextes de build, sans déployer). Un `up` ultérieur retrouve les
applications par nom et tags et leur applique le nouveau scaling sans les
recréer ; un addon existant n'est jamais recréé, un changement de plan
d'addon est signalé et reste manuel. Le `.env` du projet (ou `--env-file`)
est lu pour l'interpolation, listé dans l'éditeur et poussé avec les assets.

### Clé d'API

`--api-key CLÉ` (ou `CLEVER_COMPOSE_API_KEY`) sur `ui`/`api` exige `Authorization:
Bearer CLÉ` sur tout `/api/*` sauf `/api/info` ; l'interface la demande une
fois par onglet. Sans clé, le serveur est à réserver au poste. La CLI, elle,
se suffit à elle-même : elle parle directement à Clever Cloud, jamais à un
serveur `clever-compose api`.

### Interface déployée à part

`clever-compose api --listen 0.0.0.0:8080 --cors https://ui.exemple` sert
l'API seule (les chemins hors `/api` répondent 404) et autorise l'origine
de l'interface en CORS ; `--public-url https://api.exemple` donne la base
du callback OAuth quand un proxy est devant (c'est elle qu'on déclare comme
base URL du consumer). Le front se bâtit avec l'URL de l'API dans une
variable d'environnement Vite : `cd web && VITE_API_URL=https://api.exemple
npm run build`, puis `dist/` se sert depuis n'importe quel hôte statique.
Sans `VITE_API_URL`, le front parle à l'origine qui l'a servie, c'est-à-dire
`clever-compose ui` (voir `web/.env.example`). Le retour de la danse OAuth
revient sur l'origine de l'interface, à condition qu'elle soit listée en
`--cors`.

Le front (Vue 3 + [`@clevercloud/components`](https://www.npmjs.com/package/@clevercloud/components))
est embarqué dans le binaire, de deux façons choisies au build :

- par défaut, les sources ESM sont servies telles quelles et la page charge
  Vue, Lit et les composants depuis esm.sh / jsDelivr à l'usage (réseau
  nécessaire, pas de Node au build) ;
- `cargo build --features ui-bundled` embarque `web/dist`,
  produit par `cd web && npm install && npm run build`
  (Vite) : tout est dans le binaire, utilisable hors ligne.

## Extension `x-clever`

Au niveau projet ou service : `region`, `flavor` (ou `min_flavor` /
`max_flavor` pour laisser la plateforme changer la taille selon la charge),
`min_instances` / `max_instances` (différents : autoscaling horizontal),
`task` (service one-shot), `http_port` (port HTTP réel de
l'image), `protocol` (`http` ou `tcp`), `domains`, `ephemeral`, `plan` et
`version` (addon), `tcp_namespace` (`default` ou `cleverapps`), `private`
(addon membre du network group, vrai par défaut ; `false` = hôte public), `engine` (`documentdb`, défaut pour un `mongo` 5+ ou sans tag, ou `mongodb`), `init` (`task`, défaut : les scripts
d'initialisation d'un addon sont rejoués par une Clever Task ; `local` :
depuis le poste qui lance `up`, avec Docker s'il répond, sinon le client
`psql`/`mysql`/`mongo`/`redis-cli`/`curl` du poste).

Au niveau projet seulement : `network_groups`, exigé dès que le compose a
plusieurs réseaux — `per-network` (un network group par réseau, même
segmentation qu'en local), `single` (un seul groupe pour le projet) ou
`none` (aucun : les services ne se joignent pas entre eux).

## Ce qui est traduit

| Compose | Clever Cloud |
|---|---|
| `image:` | dépôt synthétique `FROM image` + shim ; registre privé via les identifiants du poste |
| `build:` | contexte poussé, Dockerfile prolongé (`Dockerfile.cc`) |
| `ports:` | premier port HTTP relayé sur 8080 ; un port TCP en redirection (4040, port externe attribué) ; UDP : pas disponible sur la plateforme pour le moment |
| `environment`, `env_file`, `secrets` | variables ; noms de services réécrits en FQDN de membre ; secrets matérialisés par le shim |
| `depends_on` | ordre de déploiement ; `service_completed_successfully` → Clever Task |
| `healthcheck` HTTP | `CC_HEALTH_CHECK_PATH` (2xx exigé) |
| `deploy.replicas`, `resources.limits.{memory,cpus}` | instances fixes (avertissement : pas d'autoscaling sans `x-clever`), plus petite flavor couvrant mémoire et vCPU |
| réseaux | network groups selon `x-clever.network_groups`, table DNS `CLEVER_COMPOSE_DNS_MAP` + stub DNS du shim |
| postgres, mysql/mariadb, redis/valkey, mongo (DocumentDB : PostgreSQL dédié avec l'option, API MongoDB 7.0 ; `x-clever.engine: mongodb` pour l'addon MongoDB 4.0.3), elasticsearch | addon managé sur un plan dédié, **membre du network group par défaut** : joint sur `<id>.m.<ng>.cc-ng.cloud`, port 8080 (4040 pour DocumentDB, Elasticsearch en HTTP clair), identifiants de la liaison expansés au démarrage par le shim ; `x-clever.private: false` pour l'hôte public (et le plan partagé `dev`) ; scripts d'init rejoués par une tâche (PostgreSQL, MySQL, MongoDB, DocumentDB), `seed` pour tous |
| `privileged`, `cap_add`, `devices`, `network_mode`, `sysctls`, volumes nommés… | refus avec diagnostic |

## Tests

```sh
cargo test                       # différentiel (expected.yaml figés), golden files, terraform validate
CLEVER_COMPOSE_REGEN_ORACLE=1 cargo test     # régénère les expected.yaml avec docker compose
CLEVER_COMPOSE_REGEN_GOLDEN=1 cargo test     # régénère plan.json, diagnostics et exports
```
