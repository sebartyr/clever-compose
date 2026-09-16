# clever-compose

Fait tourner un `docker compose` sur Clever Cloud : chaque service devient une
application Docker, chaque réseau un network group, les bases reconnues des
addons managés. Deux promesses : **transparence** (`clever-compose up` sans
réécrire le compose) et **sortie réversible** (export Terraform et script
clever-tools). Cadrage complet dans `CLAUDE.md`, décisions et état dans
`summary.md`.

## Installation

```sh
rustup target add x86_64-unknown-linux-musl
cargo build -p cc-compose-shim --profile shim --target x86_64-unknown-linux-musl
cargo build --release
# binaire : target/release/clever-compose
```

Le shim (PID 1 des conteneurs) est embarqué dans la CLI par `build.rs` ;
`CC_SHIM_PATH` permet d'en charger un depuis le disque en développement.

## Usage

```sh
export CLEVER_TOKEN=… CLEVER_SECRET=…    # jeton OAuth 1.0 (ou CC_API_TOKEN)
export CC_ORG=orga_xxx                   # ou --org

clever-compose plan   [--project-directory DIR] [-f FILE]... [-p NAME] [--profile P]
clever-compose up     [mêmes options] [--dry-run] [--no-wait] [--yes] [--allow-ephemeral]
clever-compose down   [-p PROJET] [--dry-run] [--yes]   (sans plan : par nom de projet et tags)
clever-compose ps     [-p PROJET]                        (apps, tâches, addons, réseaux)
clever-compose logs   [-p PROJET] [SERVICE...] [--since 1h] [-n N] [-f]
clever-compose seed   --service db --file dump.sql [--local] [mêmes options que up]
clever-compose export [terraform|script|all] -o DIR [--branches] [mêmes options que plan]
clever-compose config [mêmes options] --format json   # l'équivalent de `docker compose config`
clever-compose images --project-directory DIR         # fixture d'images pour les tests hors ligne
```

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
  PostgreSQL et MySQL, `.js`/`.archive`/`.sh` pour MongoDB et DocumentDB,
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
- Image sur un registre privé : `docker login <hôte>` sur le poste suffit,
  `up` pose `CC_DOCKER_LOGIN_*` depuis la configuration Docker et l'export
  Terraform attend `registry_<hôte>_user`/`_password` en variables.

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
| réseaux | network groups selon `x-clever.network_groups`, table DNS `CC_COMPOSE_DNS_MAP` + stub DNS du shim |
| postgres, mysql/mariadb, redis/valkey, mongo (DocumentDB : PostgreSQL dédié avec l'option, API MongoDB 7.0 ; `x-clever.engine: mongodb` pour l'addon MongoDB 4.0.3), elasticsearch | addon managé sur un plan dédié, **membre du network group par défaut** : joint sur `<id>.m.<ng>.cc-ng.cloud`, port 8080 (4040 pour DocumentDB, Elasticsearch en HTTP clair), identifiants de la liaison expansés au démarrage par le shim ; `x-clever.private: false` pour l'hôte public (et le plan partagé `dev`) ; scripts d'init rejoués par une tâche (PostgreSQL, MySQL, MongoDB, DocumentDB), `seed` pour tous |
| `privileged`, `cap_add`, `devices`, `network_mode`, `sysctls`, volumes nommés… | refus avec diagnostic |

## Tests

```sh
cargo test                       # différentiel (expected.yaml figés), golden files, terraform validate
CC_REGEN_ORACLE=1 cargo test     # régénère les expected.yaml avec docker compose
CC_REGEN_GOLDEN=1 cargo test     # régénère plan.json, diagnostics et exports
```
