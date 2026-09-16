# clever-compose

Document racine du projet, à lire intégralement au début de chaque session.
Il remplace le prompt de cadrage initial (13 sections, 5 phases), dont les
cinq phases ont été passées et vérifiées sur la plateforme le 13/09/2026. Ce
qui était hypothèse est devenu fait ; ce qui reste ouvert est listé en fin
de document. L'historique détaillé des décisions, par phase, est dans
`summary.md`, à mettre à jour à chaque changement de comportement.

---

## 1. Objectif

Un outil qui prend un `docker compose` et le fait tourner sur Clever Cloud,
chaque service devenant une application Docker distincte. Deux promesses :

1. **Transparence.** `clever-compose up` déploie sans que l'utilisateur ait à
   réécrire son compose ni à comprendre les spécificités de la plateforme.
2. **Sortie réversible.** L'outil émet la traduction en Terraform et en
   script clever-tools, pour que l'utilisateur puisse reprendre la main.

Non-objectif : reproduire toute la spécification Compose. Une bonne part des
clés est refusée avec un diagnostic clair plutôt qu'approximée.

---

## 2. Règles de travail (ne se rediscutent pas sans nouvelle source)

- **Indépendant de la CLI clever.** L'outil final n'appelle jamais `clever`
  en sous-processus et ne lit jamais `~/.config/clever-cloud/clever-tools.json`.
  Identifiants : `CC_API_TOKEN` (bearer) ou `CLEVER_TOKEN` + `CLEVER_SECRET`
  (OAuth 1.0). La CLI sert seulement aux tests manuels et au ménage.
- **Le plan intermédiaire est le contrat.** Toute évolution qui le touche se
  discute avant d'être codée (les ajouts additifs avec valeur par défaut
  absente du JSON sont tolérés, et signalés).
- **Aucune clé Compose n'est approximée.** Traduite fidèlement, ou
  diagnostic avec chemin et position dans le fichier source. L'entre-deux est
  interdit.
- **Jamais de rabattement silencieux** : version d'addon, plan, flavor,
  réseau. Un écart entre demandé et servi produit un diagnostic qui nomme les
  sorties possibles.
- **Choix explicites plutôt que défauts** (décision du propriétaire) : quand
  une alternative existe (découpage réseau, exécution locale ou tâche,
  autoscaling), l'outil demande le choix dans `x-clever` par un diagnostic
  bloquant, ou l'annonce par un avertissement, plutôt que de décider seul.
- **Vérifier la source ou tester, jamais un souvenir.** Sources locales :
  `~/axo` (API, `crates/api/networkgroup`), `~/ovd` (ancienne API),
  `~/rubydeployer` (déployeur), `~/supernova` (hyperviseur),
  `~/cc-compose/compose` (Docker Compose, compose-go dans `~/go/pkg/mod`),
  `~/cc-compose/clevercloud-sdk-rust` (SDK), `~/cc-compose/terraform-provider-clevercloud`.
- **Ce qui est fetché ou dérivé ne se code pas en dur, se relit** : schéma
  du provider Terraform, catalogue des addons, flavors, config des images
  (`data/PROVENANCE.md` dit d'où vient chaque fichier et comment le régénérer).
- **La compatibilité Terraform suit le provider.** L'export s'aligne sur le
  provider publié (version épinglée dans `data/PROVENANCE.md`). À chaque
  version : régénérer le schéma, relire `deployment`, `networkgroups`,
  `dependencies` dans ses sources, relancer `terraform validate`. Ce que le
  provider ne sait pas faire reste un avertissement d'export et une PR côté
  provider, jamais une invention ici.
- **Les manques du SDK Rust se contribuent par PR** ; en attendant, appel
  direct avec le même client signé, derrière le trait `Platform`.
- **Pas de CI GitHub Actions** (coût). Les tests se lancent en local.
- **Hôte direct des addons** (sans proxy) : jamais proposé. Un addon se
  joint par le network group (défaut, décision du 16/09/2026) ou par son
  hôte public (`x-clever.private: false`).
- Réponses en français ; commits avec le trailer de co-signature.

---

## 3. Architecture : un compilateur, pas un runtime

Le compose est parsé, normalisé, puis traduit en un **plan** (représentation
intermédiaire typée, `crates/cc-compose-model/src/plan.rs`). Trois backends :

- l'**exécuteur** (`exec/`), qui applique le plan via l'API ;
- l'**émetteur Terraform** (`emit/terraform.rs` JSON natif, `emit/hcl.rs`
  rendu lisible depuis la même structure) ;
- l'**émetteur shell** (`emit/script.rs`, bootstrap clever-tools à usage
  unique, non idempotent par nature).

L'équivalence est testée : l'exécuteur et `terraform apply` produisent les
mêmes commits déployés (vérifié en réel).

### Crates

- `cc-compose-model` : plan, config du shim, types partagés. Dépendances
  frugales, le shim en hérite.
- `cc-compose-shim` : binaire statique musl, PID 1 du conteneur. Build :
  `cargo build -p cc-compose-shim --profile shim --target x86_64-unknown-linux-musl`
  (`panic = "abort"`, LTO). Embarqué dans la CLI par `include_bytes!`
  (`build.rs` ; `CC_SHIM_BIN` au build ou `CC_SHIM_PATH` à l'exécution pour
  en charger un autre).
- `cc-compose` : parsing (`compose/`), plan (`plan/`), exécuteur (`exec/`),
  émetteurs (`emit/`), CLI (`main.rs`).

Architecture figée à amd64.

### Plan intermédiaire

`Plan { project, spec_hash, resources }`, ressources : `App` (service ou
tâche), `Addon`, `NetworkGroup`, `Membership`, `Redirection`, `Domain`.
Références symboliques (`MemberFqdn`, `AddonEnv`, `Secret`) dans les
templates d'environnement, jamais de valeur résolue ni de secret littéral.
Séparation entre l'**intrinsèque à l'image** (entrypoint, cmd, user, ports,
lus sur le registre) et le **topologique** (table DNS, secrets, addons), ce
qui rend `up` idempotent. `App.run` (`platform` | `local`) dit si une tâche
s'exécute sur la plateforme ou depuis le poste.

---

## 4. Faits plateforme (vérifiés, phase 0 et suivantes)

- Mode réseau : ports par défaut → `--net host`. **Ne jamais définir
  `CC_DOCKER_EXPOSED_HTTP_PORT` ni `CC_MOUNT_DOCKER_SOCKET`** (bascule en port
  mapping, casse la joignabilité dans le network group). Le shim relaie 8080
  vers le port réel ; TCP et UDP entrent par 4040.
- Le déployeur fait un GET HTTP sur 8080 et accepte 200 à 500 (2xx avec
  `CC_HEALTH_CHECK_PATH`). Tout conteneur doit répondre en HTTP sur 8080,
  worker et tâche compris : le shim porte un répondeur de santé.
- La plateforme n'expanse pas les variables entre elles : le shim expanse
  celles listées dans `CC_COMPOSE_EXPAND` au démarrage (identifiants d'addon
  jamais dupliqués dans la configuration ni dans l'export).
- Network groups : le nom d'un membre est imposé et déterministe,
  `<app_id>.m.<ng_id>.cc-ng.cloud`, A par peer, TTL 30, liaison à chaud sans
  redémarrage, résolution depuis le conteneur via le résolveur de la VM,
  `resolv.conf` réécrivable en place seulement. Le `domainName` posé par
  l'API est stocké mais **non résolu** (ovd#863, axo#3168) : d'où le stub DNS
  du shim (127.0.0.153, table `CC_COMPOSE_DNS_MAP` dans l'environnement, pas
  dans l'image).
- Pas de volume persistant sur le runtime Docker : un service stateful
  devient un addon ou reste éphémère sur choix explicite.
- Toutes les variables sont passées en `--build-arg`.
- Une Clever Task est une application avec `instanceLifetime: TASK` : le
  déploiement réussit quand la commande finit en 0.
- Un **plan d'addon partagé** (`dev`) sert la version de son cluster quelle
  que soit la version demandée (PostgreSQL 15 en par/mtl, MySQL 8.0 en par) :
  refus `addon-version-shared` avec les sorties (`x-clever.version` ou plan
  dédié). MongoDB n'existe qu'en 4.0.3, Redis en 8.10.1.
- **DocumentDB** : option `documentdb=true` d'un PostgreSQL dédié 17+
  (créée via `options` de l'API, pas dans le provider Terraform) ; endpoint
  compatible MongoDB 7.0 (`DOCUMENTDB_ADDON_{HOST,PORT,URI}`, user/password/db
  de `POSTGRESQL_ADDON_*`, TLS imposé avec certificat non public :
  `tls=true&tlsAllowInvalidCertificates=true`) ; l'URI apparaît quelques
  secondes après la création (l'exécuteur attend). Un `mongo` 5+ ou sans tag
  y va par défaut, un `mongo:4.x` vers l'addon MongoDB.
- Addon Elasticsearch : hôte nu servi en HTTPS seul, `ES_ADDON_URI` embarque
  les identifiants.
- **Addon membre d'un network group** : déclaré avec son **id réel**
  (`postgresql_xxx`, pas `addon_xxx`, sinon pas de peer), kind `addon` ;
  joignable sur `<realId>.m.<ng>.cc-ng.cloud` avec les identifiants de la
  liaison, sur le port interne du service (`rubydeployer/src/deploytypes/*.rb`,
  vérifié le 16/09/2026) : **8080** pour PostgreSQL, MySQL, Redis, MongoDB
  et Elasticsearch, **4040** pour la gateway DocumentDB (TLS ; 8080 est
  PostgreSQL). **Le trafic HTTP dans un network group est sans TLS**
  (propriétaire, 16/09/2026 ; le tunnel WireGuard chiffre déjà) : ES y est
  en `http://`, le HTTPS n'existe qu'au proxy public ; les URL entre
  services gardent leur schéma `http://`. Seule exception constatée : la
  gateway DocumentDB, qui impose TLS partout.
  Un plan partagé n'a pas d'instance donc pas de peer : en privé le plan
  par défaut est le moins cher dédié. L'instance d'un addon dédié met des
  minutes à rejoindre le groupe : `up` attend son peer avant de déployer
  les consommateurs, et retente un déploiement en échec. Les redirections UDP ne
  sont pas disponibles nativement pour le moment.
- Les images Redis, Valkey et Elasticsearch n'ont aucun mécanisme
  `/docker-entrypoint-initdb.d` : montage refusé, reprise par `seed`.

---

## 5. Mapping Compose vers Clever Cloud

| Compose | Traduction |
|---|---|
| `image:` | dépôt synthétique `FROM image` + shim ; registre privé = identifiants du poste pour l'hôte (`CC_DOCKER_LOGIN_*`, variables sensibles en Terraform) |
| `build:` | contexte poussé, Dockerfile prolongé (`Dockerfile.cc`, `CC_DOCKERFILE`) |
| `ports:` | premier port HTTP relayé sur 8080 ; un port TCP ou UDP en redirection (4040) ; `port-assumed-http` sinon `x-clever.protocol` |
| `expose:` | sans objet dans le groupe |
| `environment`, `env_file`, `secrets` | variables ; noms de services réécrits en FQDN de membre ; secrets matérialisés par le shim |
| `depends_on` | ordre de déploiement ; `service_completed_successfully` → tâche |
| `healthcheck` HTTP | `CC_HEALTH_CHECK_PATH` ; sinon ignoré avec avertissement |
| `deploy.replicas`, `resources.limits.{memory,cpus}` | instances fixes (`replicas-fixed` : pas d'autoscaling sans `x-clever`), plus petite flavor couvrant mémoire et vCPU |
| réseaux | network groups selon `x-clever.network_groups` (exigé dès deux réseaux) |
| postgres, mysql/mariadb, redis/valkey, mongo, elasticsearch | addon managé (coût annoncé, `--yes` au `up`), identifiants expansés par le shim ; scripts d'init rejoués par une tâche (PG, MySQL, Mongo) ; `seed` pour tous |
| `privileged`, `cap_add`, `devices`, `network_mode`, `sysctls`, volumes nommés, bind de répertoire… | refus avec diagnostic |

### Extension `x-clever`

Niveau service : `region`, `flavor` (ou `min_flavor`/`max_flavor` :
scaling vertical), `min_instances`/`max_instances` (horizontal), `task`,
`http_port`, `protocol`, `domains`, `ephemeral`, `plan`, `version`,
`tcp_namespace`, `engine` (`documentdb` | `mongodb`), `init` (`task` par
défaut, `local`), `private` (vrai par défaut : addon membre du groupe ;
`false` : hôte public). Niveau projet : `network_groups` (`per-network` | `single` |
`none`). Les clés `x-*` sont ignorées par Compose : le fichier reste
utilisable en local.

### Diagnostics à connaître

`addon-substitution`, `addon-version-unavailable`, `addon-version-shared`,
`addon-version-ambiguous`, `addon-init-task`, `addon-init-local`,
`addon-init-scripts`, `addon-tuning-args`, `addon-multiple-databases`,
`addon-no-superuser`, `addon-connections`, `addon-documentdb-shared`,
`addon-documentdb-version`, `terraform-addon-options`, `addon-private-shared`,
`addon-private-port`, `addon-private-unreachable`, `addon-private-unverified`,
`network-groups-unspecified`, `network-groups-merged`, `service-unreachable`,
`proxy-resolver`, `port-assumed-http`, `redirection-tcp-multiple`,
`replicas-fixed`, `resources-capped`, `terraform-task`, `terraform-udp`.

---

## 6. Exécuteur

Sans état local : apps nommées `<projet>-<service>`, tags `compose-project`,
`compose-service`, `compose-spec`. L'état se reconstruit depuis l'API
(`exec/inspect::discover`), ce qui sert à `up`, `down`, `ps`, `logs`.

Ordre du `up` : addons, groupes, apps sans déploiement, liaisons, variables
(FQDN calculés), redirections, domaines, puis dépôts synthétiques (git2,
commits déterministes, push sauté si l'arbre distant est identique) et
déploiements dans l'ordre topologique, en attendant chaque succès. Une tâche
`run: local` est exécutée depuis le poste à son tour (Docker si le démon
répond, sinon le client du poste).

`down` supprime par nom et tag ; les addons seulement avec `--yes` ; les apps
sans tag (créées par Terraform) sont laissées à `terraform destroy`.

---

## 7. Exports

Terraform : `main.tf.json` (validé) et `hcl/main.tf` rendu depuis la même
structure. Chaque application reçoit `networkgroups` (`fqdn` alias
`<service>.m.<ng>.cc-ng.cloud`, validé par le provider contre `cc-ng.cloud`),
`deployment { repository = var.repository, commit = "refs/heads/clever-compose/<service>" }`
(`repository` accepte `file://` : dépôt local ouvert sur place),
`dependencies` vers les ids d'addons. `export --branches` écrit les branches
dans le dépôt du projet. Aucun secret en littéral : références d'addon
expansées par le shim, secrets en `file()` ou variables sensibles.

Manques du provider (avertissements d'export, PR candidates) : lifetime Task,
redirections UDP, tags d'application.

---

## 8. Tests

```sh
cargo test                       # différentiel (expected.yaml figés), goldens du plan, exports, terraform validate
CC_REGEN_ORACLE=1 cargo test     # régénère les expected.yaml avec docker compose (oracle, jamais dépendance runtime)
CC_REGEN_GOLDEN=1 cargo test     # régénère plan.json, plan.diagnostics.txt, export/
cargo clippy --workspace --all-targets -- -D warnings   # doit passer sans rien (règle depuis le 16/09/2026)
```

Les `#[allow]` sont rares et justifiés en commentaire (`large_enum_variant`
sur `Resource`, `too_many_arguments` sur les fonctions de plan qui portent
tout le contexte, `result_large_err` sur le chargeur).

Corpus dans `crates/cc-compose/tests/corpus/<cas>/` : `compose.yaml`,
`expected.yaml` ou `expected.error`, `images.json` (fixture des configs
d'images), `plan-options.json` (`allow_ephemeral`, `private_registries`),
goldens. Cas réels (WordPress, Gitea, Nextcloud, Plausible, Immich, Mastodon,
Sentry) : ne pas modifier leur compose pour faire passer un plan ; un refus
est une sortie valide. Les cas synthétiques peuvent être annotés en
`x-clever`. Les répertoires vides du corpus portent un `.gitkeep`.

`terraform validate` tourne avec `TF_CLI_CONFIG_FILE` réduit à
`provider_installation { direct {} }` (le poste a un `~/.terraformrc` avec
`dev_overrides` et un build local du provider). Docker sur ce poste passe par
`newgrp docker <<'EOF' … EOF`. Tests réels dans l'organisation Test SBRUNAT
(`CC_ORG=orga_cd3ca43d-…`), avec des identifiants exportés dans un `env.sh`
du scratchpad, jamais lus par l'outil ; tout projet de test se détruit après
usage (`down --yes`, `terraform destroy`).

---

## 9. Commandes

```
clever-compose config | plan | images | export [terraform|script|all] -o DIR [--branches]
clever-compose up   --org ORG [--dry-run] [--no-wait] [--yes] [--allow-ephemeral]
clever-compose seed --service SVC --file DUMP [--local]
clever-compose down --org ORG [-p PROJET] [--dry-run] [--yes]
clever-compose ps   --org ORG [-p PROJET]
clever-compose logs --org ORG [-p PROJET] [SERVICE...] [--since 1h] [-n N] [-f]
```

---

## 10. Ouvert, reporté, décidé

- Reporté : DocumentDB dans l'export Terraform (pas d'attribut d'option
  dans le provider : ressource omise, id réel en variable
  `addon_<svc>_real_id`, `terraform-addon-options`) ; les autres points
  « choix vs défaut » proposés et non retenus (service à service par domaine
  public, protocole exigé, addon public ou groupe).
- SDK Rust : PR #81 (network groups) à rebaser, bug `v2::plan::list` sur
  `/v2/products/addonproviders`, routes applications à contribuer.
- Provider Terraform : PR candidates ci-dessus (§7). Pas de contribution
  externe pour le moment (décision du 16/09/2026).
- Le backlog vit dans les issues GitHub du dépôt (`gh issue list`), labels
  `reporté`, `plateforme`, `validation`, `produit` ; ce document et
  `summary.md` n'en gardent que la synthèse.
- Résolution du `domainName` par le DNS des network groups : commentaire
  posté sur axo#3168 ; si elle aboutit, le stub DNS devient inutile au profit
  d'un `search` dans `resolv.conf`.
