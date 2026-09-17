# summary.md : journal des décisions

Mis à jour à la fin de chaque phase (règle anti-dérive, CLAUDE.md §13).
Trois rubriques par phase : décidé, invalidé, questions ouvertes.

---

## État courant

**Phase 0 terminée le 13/09/2026.** Critère de sortie atteint : `postgres:16`
piloté par le shim est joignable sur 5432 depuis une autre application du
même network group, et les sept questions de la section 4 ont une réponse
écrite (tableau ci-dessous, détail dans la section Phase 0).

Workspace Rust posé (trois crates). Shim minimal (PID 1, exec, service du port
8080), compilé en statique musl via
`cargo build -p cc-compose-shim --profile shim --target x86_64-unknown-linux-musl`.
Fixtures `phase0/postgres` et `phase0/client` déployées à la main dans
l'organisation Test SBRUNAT (apps `cc-phase0-postgres`, `cc-phase0-client`,
network group `p0`), à supprimer quand elles ne servent plus.

**Phase 1 terminée le 13/09/2026** : le corpus (15 cas dont un refus) passe
le test différentiel, chaque clé refusée produit un diagnostic avec chemin
et position, le plan sort en golden files. Le plan intermédiaire reste
soumis à validation. **Phase 2 terminée le 13/09/2026** : `terraform
validate` passe sur les exports du corpus (test `corpus_terraform_validates`,
terraform 1.16.2 dans `~/.local/bin`), script de bootstrap relu sur
WordPress, secrets, redirections et un projet `build:` (exécuté en local
jusqu'au conteneur). **Phase 3 terminée le 13/09/2026** : `clever-compose
up` sur un compose à deux services (`ccp3`, Test SBRUNAT) déployé, puis
redéployé sans changement avec les deux push sautés. **Phase 4 terminée le
13/09/2026** : WordPress (`ccwp`, Test SBRUNAT) fonctionne sans modification
du fichier source (`--allow-ephemeral` en option), MariaDB jointe par le
FQDN du membre. **Phase 5 : 5a, 5b et 5d faites le 13/09/2026** (WordPress
`ccwpa` sur un addon MySQL `dev`, identifiants jamais dans la configuration,
expansés par le shim). **5c faite le 13/09/2026** : tâche d'initialisation
et `seed` vérifiées sur `ccinit` (PostgreSQL 16 `dev`). **Toutes les phases
du cadrage sont passées ; validation sur cas concrets à faire par le
propriétaire.**
Dépôt déplacé le 13/09/2026 dans `~/cc-compose/clever-compose`, avec les
sources de Compose dans `~/cc-compose/compose`, Axo dans `~/axo`, OVD dans
`~/ovd`. Remote `origin` : `github.com/sebartyr/clever-compose`.
CI GitHub Actions retirée le 13/09/2026 à la demande du propriétaire (elle
avait servi à corriger deux portabilités du corpus : chemins absolus dans
`expected.yaml` remplacés par `<project>`, répertoires vides suivis par
`.gitkeep`). Les tests se lancent en local avec `cargo test`.

## Références

- [clever-kubernetes-operator](https://github.com/CleverCloud/clever-kubernetes-operator) :
  comment l'opérateur Kubernetes consomme les services managés du PaaS (une
  PostgreSQL par exemple) et câble les identifiants. À relire en phase 5.
- [terraform-provider-clevercloud](https://github.com/CleverCloud/terraform-provider-clevercloud) :
  `pkg/resources/ng.go` porte la logique de synchronisation des membres.
- Monorepo Axo (sources de l'API, local : `~/axo`) : `crates/api/networkgroup`
  est l'API des network groups (`controller.rs` pour les routes,
  `dns_handler.rs` pour le DNS servi, `views.rs` pour les modèles).

## Accès plateforme

- **Contrainte (13/09/2026) : l'outil final est indépendant de la CLI clever.**
  Pas de `clever` en sous-processus, pas de lecture de
  `~/.config/clever-cloud/clever-tools.json`. L'exécuteur (phase 3) passe par
  le SDK Rust ou `reqwest` avec une authentification qui lui est propre (token
  d'API ou jeton OAuth 1.0 fourni à l'outil). Le script clever-tools reste un
  export, jamais une dépendance.
- Pour l'expérimentation manuelle de phase 0, clever-tools 5.0.0 est installé
  et connecté ; c'est légitime tant qu'il n'y a pas d'outil.

---

## Questions ouvertes (CLAUDE.md §4)

| # | Question | Statut | Réponse |
|---|---|---|---|
| 1 | Alias de nom (`fqdn` du bloc `networkgroups` Terraform) | **répondue** | L'API accepte et stocke un `domainName` arbitraire sous `cc-ng.cloud`, **mais le DNS ne le sert pas**. Lecture « champ ignoré ». Le stub DNS reste nécessaire. |
| 2 | FQDN lisible en Terraform pour un apply en une passe | **répondue** | Le nom servi est déterministe : `<app_id>.m.<ng_id>.cc-ng.cloud`, calculable en HCL depuis `clevercloud_docker.x.id` et `clevercloud_networkgroup.y.id`. Apply en une passe pour un graphe de dépendances acyclique ; les références mutuelles entre services font un cycle Terraform (à traiter en phase 2). |
| 3 | TTL et enregistrements A de `cc-ng.cloud` | **répondue** | Un A par peer vivant, **TTL 30**. AAAA : NOERROR sans réponse (NODATA). Pendant un redéploiement, l'ancienne et la nouvelle instance apparaissent toutes deux. |
| 4 | Liaison à chaud : redémarrage nécessaire ? | **répondue** | **Non.** Liaison à 15:23:55, résolution et connexion réussies à 15:24:00 sans redémarrage d'aucune des deux apps. |
| 5 | Résolution `cc-ng.cloud` en `--net host`, `resolv.conf` modifiable ? | **répondue** | Oui, via le résolveur de la VM (`nameserver 10.2.0.1`, puis `1.1.1.1`, `search .`). Zone privée, pas résolvable depuis l'extérieur. `resolv.conf` est un bind mount : **réécrivable en place, pas remplaçable** (`rm` refusé). |
| 6 | Workers sans port : critère de succès du déploiement | **répondue** | Le déployeur fait un **GET HTTP sur 8080** et accepte tout code 200 à 500. Sans écoute : échec après 4 min. Réponse non HTTP : plantage du déployeur, échec. Chaque conteneur doit répondre en HTTP sur 8080, worker compris. |
| 7 | Expansion des variables entre elles par la plateforme | **répondue** | **Non.** `P0_REF=ref=${P0_PASSWORD}/end` arrive littéralement dans le conteneur. L'expansion par le shim (§6.2) est nécessaire. |

---

## Phase 5 : substitution par les addons

*Sortie 5a/5b/5d :* atteinte le 13/09/2026. Aucune substitution silencieuse :
chaque substitution est un avertissement `addon-substitution` avec plan,
prix mensuel et limite de connexions, et `up` exige `--yes` avant de créer
un addon. Version indisponible ou image construite : jamais substituées.
Les consommateurs du corpus (WordPress) se connectent sans intervention.

*Décidé :*

- **Catalogue embarqué** (`data/addon-providers.json`, provenance dans
  `data/PROVENANCE.md`) : plans avec id, slug, prix, zones, features (`Max
  connection limit`), versions dédiées et version par défaut, pour
  `postgresql-addon` (13 à 18, défaut 17 au 13/09), `mysql-addon` (5.7,
  8.0, 8.4, 9.7), `redis-addon`, `mongodb-addon`, `es-addon`,
  `cellar-addon`, `config-provider`.
- **Reconnaissance** (`plan/addons.rs`) : image → fournisseur **et**
  dialecte (officiel ou Bitnami), avec variantes (`postgis`, `pgvector`,
  `timescale`, `valkey`, `docker.elastic.co/...`). Version depuis le tag
  (majeure pour PostgreSQL, Redis, ES ; `majeure.mineure` pour MySQL,
  MongoDB). **MariaDB** : le fournisseur est MySQL, la version demandée
  n'existe pas → erreur qui exige `x-clever.version` (choix explicite) ou
  `x-clever.ephemeral: true`. `--allow-ephemeral` ne concerne que les
  volumes des services non substituables ; seul `x-clever.ephemeral` garde
  une base en conteneur.
- **Plan** : le moins cher servi dans la région (`dev` gratuit pour
  PostgreSQL et MySQL), `x-clever.plan` pour choisir.
- **Câblage** (5b) : URL entière reconstruite depuis les variables de
  l'addon (`redis://default:${REDIS_PASSWORD}@…` pour Redis, §6.2 cas 3) ;
  hôte réécrit en `${X_ADDON_HOST}` (`host:port` → `${HOST}:${PORT}`) ;
  variables discrètes corrélées par **égalité de valeur** avec les
  identifiants déclarés, la clé levant les ambiguïtés (`PASS`, `USER`,
  `DB`/`DATABASE`/`NAME`, `PORT`), rien si ambigu ; `CC_COMPOSE_EXPAND`
  liste les variables que le shim expanse au démarrage (question 7 de la
  phase 0). Les consommateurs dépendent de `addon/<service>` ; l'addon est
  hors network group (§6.4) et absent de la table DNS.
- **Falaises** (5d), chacune nommée : `addon-tuning-args` (erreur),
  `addon-multiple-databases` (erreur), `addon-init-scripts` (erreur tant
  que 5c n'existe pas), `addon-no-superuser` (avertissement),
  `addon-connections` (avertissement, instances × 10 contre la limite du
  plan).
- **Exécuteur** : addon retrouvé par nom, créé via le SDK
  (`v2::addon::create`, id de plan relu sur `/v2/products/addonproviders`
  en JSON libre car `v2::plan::list` du SDK échoue à désérialiser cette
  route : bug à remonter), liaison `POST …/applications/{app}/addons` (corps
  : l'id en chaîne JSON), redémarrage si une liaison est nouvelle.
- **Émetteurs** : Terraform `clevercloud_{postgresql,mysql,redis,mongodb}`
  et `dependencies` sur l'application (validé) ; script `clever addon
  create` + `clever service link-addon`.

- **5c, scripts d'initialisation et reprise** (`plan/initdb.rs`) : les
  fichiers montés sur `/docker-entrypoint-initdb.d` (fichier ou répertoire,
  `.sql`, `.sql.gz`, `.sh`, ordre alphabétique) deviennent une Clever Task
  `<service>-init` (image `postgres:<version>` ou `mysql:<version>` pour le
  client, scripts copiés dans `/cc-init`, script d'exécution généré dans le
  plan, `Source::Synthetic.generated`). Table témoin `cc_compose_init`
  (hash du contenu) : rejoué seulement si le contenu change ; les
  consommateurs dépendent de la tâche. Vérifié : deux scripts appliqués,
  redéploiement forcé → « déjà appliqués », `up` suivant sans push.
  `clever-compose seed --service db --file dump.sql` : même tâche
  (`<service>-seed`) avec le dump, même témoin. Une tâche est un déploiement
  `OK` quand son processus sort en 0 (rubydeployer `run_app` sans contrôle
  de santé).

*Questions ouvertes restantes :*

- Scripts d'initialisation pour Redis, MongoDB, Elasticsearch : non
  supportés (erreur explicite).
- **Redis vérifié en réel** (`ccredis`, plan `s_mono`, 13/09/2026) :
  `REDIS_URL=redis://:pass@cache:6379/0` reconstruit en
  `redis://default:${REDIS_PASSWORD}@${REDIS_HOST}:${REDIS_PORT}/0`,
  `CACHE_HOST`/`CACHE_PASSWORD` corrélés, `AUTH` puis `PING` → `+PONG`
  depuis une tâche. `--requirepass` dans la `command` n'est plus pris pour
  un réglage. Piège de test : `$VAR` dans une `command` est interpolé par
  Compose au chargement (`$$` pour un littéral), comme avec docker compose.
- **Gitea vérifié en réel** (`ccgitea`, PostgreSQL 17 `dev`) :
  `DB_HOST=db:5432` → `${POSTGRESQL_ADDON_HOST}:${POSTGRESQL_ADDON_PORT}`,
  `/api/healthz` en `pass`.
- Ménage fait le 13/09/2026 : l'organisation Test SBRUNAT est vide (apps,
  addons, network groups, `ccnginxtf` via `terraform destroy`, phase 0).
  Reste au propriétaire : supprimer le dépôt GitHub jetable
  `sebartyr/cc-compose-tftest` (le token `gh` n'a pas le scope `delete_repo`).
- **Choix réseau explicite (décision du propriétaire, 13/09/2026)** : « laisser
  le choix pour certains points à alternative, notamment le réseau, et
  arrêter des choix par défaut », sous forme de diagnostic bloquant + clé
  `x-clever`. Point retenu : le découpage en network groups.
  `x-clever.network_groups` au niveau projet, exigé dès que le compose a
  plus d'un réseau (`network-groups-unspecified`) : `per-network` (un groupe
  par réseau, comportement précédent), `single` (un seul groupe
  `<projet>-default`, avertissement `network-groups-merged`), `none` (aucun
  groupe, les références entre services restent en `service-unreachable`).
  Un seul réseau : pas de question, `per-network` et `single` sont
  identiques. Corpus : `networks-unspecified`, `networks-single` ; Mastodon
  et override-rules déclarent `per-network`. Non retenus pour l'instant :
  service à service par domaine public plutôt que par NG, protocole exigé
  au lieu de supposé HTTP, addon hôte public ou NG.
- `ps` et `logs` ajoutés le 13/09/2026 à la demande du propriétaire, sans
  plan comme `down` (`exec/inspect.rs`) : `ps` liste apps, tâches (état des
  instances ou dernier déploiement, premier domaine), addons et réseaux ;
  `logs` lit la route SSE v4 (`/v4/logs/organisations/{o}/applications/{id}/logs`,
  `resources/{id}/logs` pour les addons ; `since`/`until` RFC 3339, `limit`,
  `END_OF_STREAM`), un flux par ressource, lignes préfixées du service,
  triées par date hors `-f`. Constaté sur les logs de la plateforme : stdout
  et stderr du conteneur ne sont pas ordonnés entre eux à la milliseconde
  près.
- Au passage, `down` recalculait un plan et échouait dès qu'un diagnostic
  bloquait (option `--allow-ephemeral` oubliée, chemin relatif) : il ne
  supprimait alors rien. Réécrit sans plan : nom de projet (`-p`,
  `COMPOSE_PROJECT_NAME`, compose s'il se charge, sinon répertoire) ; apps et
  network groups par préfixe `<projet>-` **et** tag `compose-project=`,
  addons par préfixe et fournisseur connu (pas de tags sur les addons),
  supprimés seulement avec `--yes`. Conforme au « pas d'état local » de §7.
- Latence du proxy / hôte direct (§6.4) : **décision du propriétaire
  (13/09/2026) : on ne propose pas l'hôte direct.** Un addon se joint soit par
  son hôte public (variables injectées par le lien), soit via le network group
  (`x-clever.private: true`). Point clos, à ne pas rouvrir.
- Noms des variables injectées confirmés par la page de référence de la
  documentation (`environment-variables/reference.md`) et le test e2e Redis
  d'Axo (`REDIS_URL` = `redis://:pass@host:port`) : `MONGODB_ADDON_*`,
  `ES_ADDON_*`, `REDIS_*`. MongoDB et Elasticsearch sont des fournisseurs
  hors OVD/Axo (API à part).
- **MongoDB, Elasticsearch et Redis exercés en réel le 13/09/2026**
  (`ccmongo`, `cces`, `ccredis2`, détruits ensuite) :
  - MongoDB (seule version servie : 4.0.3) : tâche d'init avec le shell
    `mongo` de l'image `mongo:4.0`, scripts `.js`/`.sh`, collection témoin
    `cc_compose_init` ; `seed` en `.js`, `.archive` (mongorestore) ou `.sh`.
    Vérifié : `01-users.js` rejoué, sonde lisant `alice`/`bob`, seed d'un
    second script.
  - Elasticsearch : l'image n'exécute aucun script d'initialisation, un
    montage vers `/docker-entrypoint-initdb.d` est refusé
    (`addon-init-scripts`) ; la reprise passe par `seed` en `.ndjson`
    (`_bulk` via `curlimages/curl`, index témoin `cc_compose_init`).
    Constaté : `ES_ADDON_HOST` est un hôte nu servi en HTTPS seul, et
    `ES_ADDON_URI` embarque les identifiants ; les URL des consommateurs sont
    donc reconstruites en `https://user:pass@host` quel que soit le schéma
    d'origine. Vérifié : bulk chargé, sonde trouvant le document, rejeu de la
    tâche arrêté par le témoin.
  - Redis (Valkey compris) : même absence de mécanisme d'init dans l'image,
    refus du montage ; `seed` en `.redis` (commandes, `redis-cli <`) ou
    `.resp` (`--pipe`), clé témoin `cc_compose_init:<hash>`. Vérifié :
    `GET greeting` → `bonjour` via `REDIS_URL` réécrite.
- Une version de tag moins précise que celle servie (`8` pour `8.10.1`,
  `4.0` pour `4.0.3`) désigne la seule qui la prolonge ; plusieurs
  candidates donnent `addon-version-ambiguous`.
- **Addons membres du network group par défaut (décision du propriétaire,
  16/09/2026 : « je veux que ce soit via NG par défaut »).** Constaté sur la
  plateforme : le membre se déclare avec l'**id réel** (`postgresql_xxx`,
  `mysql_xxx`, `redis_xxx` ; avec `addon_xxx` le membre est accepté mais
  n'a jamais de peer), kind `addon` ; le service répond sur
  `<realId>.m.<ng>.cc-ng.cloud:8080` (port indiqué par le propriétaire,
  vérifié : PostgreSQL 16.15, MySQL 8.4, Redis PONG) avec les identifiants
  de la liaison. Un plan partagé n'a pas de VM donc pas de peer. Plan :
  `Addon.private` (vrai sauf `x-clever.private: false`, forcé faux pour
  Mongo/ES non vérifiés avec `addon-private-unverified` si demandé),
  `Membership` avec `member: addon/<svc>` sur les réseaux du service, plan
  par défaut = moins cher **dédié** (`addon-private-shared` si un plan
  partagé est imposé), câblage : hôte = `MemberFqdn` du membre, port = 8080
  (`ADDON_NG_PORT` dans le modèle), user/password/db = variables de la
  liaison ; `CC_COMPOSE_DNS_MAP` porte l'alias ; `addon-private-port`
  quand l'application ne reçoit qu'un hôte sans port (elle supposerait le
  port par défaut). Exécuteur : `AddonInfo.real_id`, `link_member(kind)`.
  Terraform : `networkgroups` sur la ressource addon, FQDN depuis `.id`
  (id réel). Script : `clever ng link "$ADDON_REAL_x"`. Vérifié en réel :
  WordPress (`WORDPRESS_DB_HOST: db:3306` → `mysql_…:8080`) sur MySQL
  `xxs_sml` membre du groupe, page d'installation servie, second `up`
  sans changement. Effet de bord : le plan dédié par défaut sert la version
  demandée, Mastodon/Plausible/Sentry ne sont plus bloqués par
  `addon-version-shared`.
- **DocumentDB (16/09/2026, demande du propriétaire : « au moins le support
  partiel via API, tant pis pour Terraform »).** Source : la doc
  (`~/documentation/content/doc/deploy/databases/documentdb.md`) et
  clever-tools (`--option documentdb=true` → `options.documentdb: "true"`
  de `POST /addons`). Moteur interne `documentdb` (`addons::DOCUMENTDB`) :
  fournisseur plateforme `postgresql-addon`, plan dédié imposé, version
  PostgreSQL 17+ (`x-clever.version` = version PG), `Addon.options`
  (nouveau champ additif). Un service `mongo` 5+ ou sans tag y va par défaut
  (l'addon MongoDB n'existe qu'en 4.0.3, DocumentDB annonce 7.0), un 4.x
  vers MongoDB ; `x-clever.engine` force. Câblage : hôte/port
  `DOCUMENTDB_ADDON_*`, user/password/db `POSTGRESQL_ADDON_*`, URL
  reconstruite avec `tls=true&tlsAllowInvalidCertificates=true` (certificat
  hors autorités publiques). Init/seed : image `mongo:7`, `mongosh --tls
  --tlsAllowInvalidCertificates`, mongorestore `--ssl`. Exécuteur : création
  par requête brute (le SDK ne porte pas d'options libres), attente de
  `DOCUMENTDB_ADDON_URI` (apparaît après la création : lier avant, c'est
  déployer sans l'URI, constaté). Script : `--addon-version` (l'ancien
  `--version` était faux) et `--option documentdb=true`. Terraform : pas
  d'attribut d'option dans le provider 2.2.0 → ressource omise et
  dépendances retirées, `terraform-addon-options` ; manque suivi en issue.
  Vérifié en réel (`ccddb`, détruit) : `db.version()` = 7.0.0, script
  d'init rejoué, documents lus par l'URL réécrite. Non vérifié : accès par
  le network group (port du gateway inconnu, `private` forcé faux).
- **Issue #1 (16/09/2026) : accès par le groupe vérifié pour MongoDB,
  Elasticsearch et DocumentDB.** Ports internes lus dans
  `~/rubydeployer/src/deploytypes/{mongodb,elasticsearch,postgresql}.rb`
  (indication du propriétaire) puis sondés : MongoDB 8080, Elasticsearch
  8080 en HTTP clair (TLS côté proxy public seulement), gateway DocumentDB
  4040 en TLS (`GatewayListenPort`, 8080 étant PostgreSQL). Modèle :
  `addon_ng_port(engine)` remplace la constante ; ES reconstruit en
  `http://` dans le groupe ; `NG_VERIFIED` couvre les six moteurs, donc
  `private` vaut vrai par défaut pour tous. Deux défauts de l'exécuteur
  corrigés au passage : les consommateurs étaient déployés avant que
  l'instance d'un addon dédié tout juste créé ait rejoint le groupe (ES met
  plusieurs minutes) → attente du peer du membre (`ng_members_with_peer`,
  15 min max) ; un dernier déploiement en `FAIL`/`CANCELLED` n'était pas
  retenté quand rien n'avait changé → redéploiement. Terraform : un addon
  créé hors Terraform (options) est désigné par la variable
  `addon_<svc>_real_id` (FQDN de membre et `dependencies`). Vérifié en réel
  (`ccng4`, détruit) : `mongodb 4.0.3`, `documentdb 7.0.0`, ES `8.19.9`
  atteints par les URL réécrites (`m:27017` → `…:8080`, `d:27017` →
  `…:4040` + TLS, `http://es:9200` → `http://…:8080`).
- **Retours du premier essai du propriétaire sur son propre compose
  (16/09/2026)** : (1) espace personnel `user_…` : `PUT /v2/self/applications/{id}/tags`
  répond 405 — sur Axo les tags se posent un par un
  (`PUT`/`DELETE …/tags/{tag}`), le `PUT` de la liste n'existait que sur
  l'ancienne route des organisations ; `set_tags` fait maintenant la
  différence tag par tag (vérifié sur `user_…`). (2) Affichage : module `ui`
  — couleurs sur un terminal (`NO_COLOR` respecté), diagnostics mis en forme
  (position, sévérité, code, chemin), ligne vide entre diagnostics et
  actions, verbe coloré selon son sens, indicateur de progression en place
  pendant les attentes (déploiement, peer d'addon, endpoint DocumentDB ;
  une seule ligne hors terminal), bilan final ✔/✘.
- **Homonymes (16/09/2026)** : le nom n'est pas unique sur la plateforme.
  `up` choisit désormais parmi les homonymes celle qui porte le tag
  `compose-project` (un homonyme sans tag est signalé et laissé), refuse
  s'il y en a plusieurs avec le tag (ids listés), même règle pour les
  network groups (label + tag) et les addons (nom + fournisseur). Vérifié en
  réel avec un homonyme étranger puis un doublon tagué.
- `down --all` : supprime tout ce que clever-compose a créé dans
  l'organisation (projets retrouvés par le tag), addons avec `--yes`.
- Fait plateforme (propriétaire, 16/09/2026) : **le trafic HTTP dans un
  network group est sans TLS** (WireGuard chiffre le tunnel). Cohérent avec
  ce qui est constaté : ES en `http://…:8080` dans le groupe, HTTPS au proxy
  public seulement ; DocumentDB reste l'exception (gateway TLS sur 4040).
  Une URL `https://<service>` entre services du compose garderait son
  schéma et échouerait : pas de diagnostic dédié aujourd'hui.
- **Interface web (16/09/2026, demande du propriétaire : « très visuel,
  diagramme des ressources avec les détails dessus, toutes les features de
  la CLI, modification du compose, chargement d'un projet par archive »,
  jetons OAuth saisis dans le front, liste des organisations en CLI et en
  UI).** `clever-compose ui [--listen]` : serveur `axum` embarqué
  (`src/web/`), API JSON locale sans autre logique que celle de la CLI
  (`plan::cli::run`, `reconcile::{up,down}`, `inspect`, `emit::export`),
  opérations longues en SSE (`log` puis `done`), projets locaux
  (`web/local.rs` : répertoire, ou archive zip/tar extraite dans un temp
  supprimé au retrait ; chemins d'archive bornés au répertoire), session
  mutable (identifiants d'environnement au lancement ou saisis, vérifiés
  par `/v2/self`, jamais écrits ; organisation choisie dans la liste de
  `Api::organisations()` = `/v2/self` + `/v2/organisations`). CLI :
  `orgs`. Front `web/` : Vue 3 avec templates de chaînes (pas de SFC, mêmes
  sources dans les deux modes) + composants Lit `cc-*` (button, badge,
  block, dialog, input-text, loader, notice, select, toggle ; événements
  `cc-click`, `cc-input`, `cc-select`, `cc-multi-select`, `cc-close`,
  `cc-notice-dismiss`) ; diagramme SVG maison (`graph.js` : modèle commun
  plan/inventaire, boîtes par network group, colonnes par profondeur de
  dépendance ; `diagram.js`), détail au clic, éditeur, diagnostics, export,
  journal avec progression remplacée en place, liens directs
  `?project=`/`?local=`. Deux modes de build : feature `ui-bundled`
  (`web/dist` de Vite, `rust-embed`) sinon sources ESM + import map (Vue
  esm.sh, `lit` esm.sh en un seul exemplaire, composants bruts jsDelivr —
  esm.sh les regroupait et `cc-icon` était défini deux fois). Vérifié :
  rendu identique dans les deux modes (Chromium headless), `up` par l'API
  en SSE, inventaire avec instances, upload/extraction/retrait d'archive,
  clippy strict dans les deux modes.
- **`stop` / `start` (16/09/2026)** : « si je down et que je veux pas
  supprimer ? et si je veux up un truc éteint ? ». `stop` = `DELETE
  …/instances` (les applications s'arrêtent, configuration, addons et
  groupes restent), `start` = redéploiement des applications arrêtées ;
  `up` relance aussi un service arrêté (état `SHOULD_BE_DOWN` avec un
  déploiement passé), pas une tâche (elle a fini). CLI, API (`/stop`,
  `/start` en SSE) et boutons de l'interface. Vérifié en réel.
- Interface, retours du propriétaire : les boutons des dialogues étaient
  dans un `slot="footer"` que `cc-dialog` n'a pas (seul un slot par défaut
  et `heading`) → invisibles ; corrigé, testé avec Puppeteer. Journal :
  verbes colorés comme dans le terminal et séquences ANSI SGR des logs
  applicatifs rendues. `~` accepté dans « ouvrir un répertoire », refus
  clair si le répertoire n'a pas de compose.
- **Assets dans Cellar (16/09/2026)** : demande « un bucket Cellar pour
  stocker les assets uploadés d'un projet, vue plus complète d'un projet
  déployé », puis consignes « laisse la personne créer l'addon », « un
  seul bucket », « tu touches pas à ça », « via var d'env à configurer ».
  `x-clever.assets: true` (niveau projet, `Plan.assets`) ou `up --assets` ;
  configuration `CC_ASSETS_BUCKET` + `CELLAR_ADDON_HOST`/`KEY_ID`/
  `KEY_SECRET` (noms injectés par l'addon), un seul bucket, préfixe
  `<projet>/` ; l'outil ne crée ni addon ni bucket (erreur claire si le
  bucket n'existe pas). `up` pousse les fichiers texte du projet (≤ 512
  Kio, hors `.git`/`node_modules`/`target`/`.terraform`/`.clever-compose`)
  plus tout fichier référencé par le plan (≤ 32 Mio), `plan.json` et
  `manifest.json` (sha256, tailles, date) ; inchangé = sauté, disparu =
  retiré ; `down` retire les objets du projet, jamais le bucket. Client
  S3 : `rusty-s3` (URL présignées, path-style). API `GET …/assets`, `POST
  …/assets/open` (rapatriement en projet local temporaire) ; interface :
  onglet « Fichiers (Cellar) », « Ouvrir en local », plan du bucket en
  diagramme. Vérifié en réel avec la première version (bucket dédié créé
  par l'outil, retirée) ; la version finale compile et passe les tests, à
  vérifier par le propriétaire avec son bucket.
- **Session de l'interface (16/09/2026)** : « on va retirer les jetons à
  set à la main. Soit c'est présent dans l'env soit le lancement de l'ui
  échoue sauf si la danse oauth est conf ». `ui` démarre avec les
  identifiants d'environnement, ou avec un consumer OAuth 1.0
  (`CLEVER_CONSUMER_KEY`/`CLEVER_CONSUMER_SECRET`, créé par le propriétaire
  dans la console, base URL = adresse d'écoute) : `GET
  /api/session/oauth/start` → `POST /v2/oauth/request_token_query`
  (signature `PLAINTEXT`, admise par la doc de l'API, transport TLS) →
  redirection `/v2/oauth/authorize` → `/api/session/oauth/callback` →
  `POST /v2/oauth/access_token_query` → session. Sans l'un ni l'autre,
  refus au lancement. Vérifié : étape 1 avec le consumer du propriétaire
  (la clé lue par `/v2/self/consumers`, l'OCR de la capture confondait `I`
  et `l`) ; étapes 2-3 à faire dans un navigateur connecté. Choix de
  l'interface (options, logs, seed local, dernier projet, onglet,
  organisation) en `localStorage`. `cc-toggle` rétabli pour les options.
  Vérifié ensuite par le propriétaire de bout en bout (« ok ça marche »).
- **Serveur exposé : deux garde-fous (17/09/2026)** : discussion sur
  l'utilité de la clé d'API (« si on doit s'identifier avec ses jetons de
  toute manière on va déployer sur ses orga ») : elle ne protège que la
  machine, pas les organisations ; en interface statique elle ne peut
  être que saisie. Fait : (1) les identifiants du serveur ne sont prêtés
  aux clients sans jetons qu'en écoute sur une adresse de bouclage
  (sinon ignorés, journalisé) ; (2) chaque projet local a un propriétaire
  (id Clever Cloud du client à jetons, `None` pour le serveur), `Locals`
  filtre par propriétaire, « ouvrir un répertoire du poste » est refusé
  (403) à un client à jetons et masqué dans l'interface. Clé conservée en
  option. Vérifié : sur `0.0.0.0` avec `CLEVER_COMPOSE_API_TOKEN`, repli
  ignoré et 401 pour un client sans jetons.
- **Préfixe `CLEVER_COMPOSE_` (17/09/2026)** : « prefix tout avec
  CLEVER_COMPOSE plutôt que CC_ ». Toutes nos variables changent de nom :
  `CLEVER_COMPOSE_{API_TOKEN, ORG, API_KEY, LISTEN, ASSETS_BUCKET,
  SHIM_PATH, SHIM_BIN, SHIM_EMBEDDED, BUILD_STAMP, IMAGE_CACHE_TTL,
  REGEN_ORACLE, REGEN_GOLDEN}` et, dans les conteneurs, `CLEVER_COMPOSE_{
  DNS_MAP, EXPAND, INIT_DIR, INIT_HASH, SECRET_*}` (ex-`CC_COMPOSE_*`,
  `CC_INIT_DIR`). Celles de la plateforme (`CC_HEALTH_CHECK_PATH`,
  `CC_DOCKERFILE`, `CC_DOCKER_LOGIN_*`, `CC_DOCKER_EXPOSED_*`,
  `CC_MOUNT_DOCKER_SOCKET`) gardent évidemment leur nom. `CLEVER_TOKEN`/
  `CLEVER_SECRET`/`CLEVER_CONSUMER_*` restent (convention clever-tools).
  Goldens régénérés (noms et alignement HCL seulement).
- **Port 8080 par défaut (17/09/2026)** : `ui`/`api` écoutent sur
  `127.0.0.1:8080` (`--listen` ou `CC_LISTEN`), `login --callback-port`
  8080 : le port attendu par une application Clever Cloud, et la base
  URL du consumer sur le poste devient `http://127.0.0.1:8080/`.
- **Interface hébergée sur Clever Cloud (17/09/2026)** : capture du
  propriétaire, `ui` lancé dans une application cleverapps.io refusait
  `return=https://app-….cleverapps.io/` (« ni un chemin de ce serveur ni
  une origine --cors ») : la base publique était `http://0.0.0.0:8080`.
  Désormais `State::public_base_for(headers)` : `--public-url` si donnée,
  sinon `X-Forwarded-Proto` + `X-Forwarded-Host`/`Host` de la requête ;
  le callback OAuth en découle. Le front envoie un retour relatif quand
  l'API sert la page (absolu seulement avec `VITE_API_URL`). Le
  propriétaire a passé la base URL du consumer embarqué à l'application
  cleverapps.io : vérifié, request token accepté avec l'hôte de l'app
  transmis, refusé pour `127.0.0.1` (la base URL décide d'où la danse se
  fait ; un autre consumer par `CLEVER_CONSUMER_*` pour le poste).
- **Serveur verbeux (17/09/2026)** : « rends l'utilisation de l'ui/api
  verbeuse ». `web::slog` (stderr, horodaté) : au démarrage, mode
  d'identifiants, consumer (environnement ou embarqué), CORS, clé, bucket ;
  une ligne par requête `/api/*` (méthode, chemin, statut, durée, jetons
  du client ou identifiants du serveur, organisation) par le middleware
  `log_request` ; jetons validés (email) ou refusés (raison) ; connexion
  (request token, jetons obtenus pour qui) ; projets reçus/retirés ;
  opérations SSE `▶ up …` puis chaque ligne du journal et `✔`/`✘` avec la
  durée (`stream_task` prend un libellé).
- **Consumer clever-compose embarqué, CLI autonome (17/09/2026)** : « pose
  les consumers dédiés à clever compose dans le code par défaut », « on
  override tjs avec les var d'env », « la cli se suffit à elle même,
  retire la possibilité d'utiliser une api distante », « le front lui
  peut se connecter à une api distante mais dans ce cas faut fournir
  l'URL », « le front passe les user tokens à chaque requête », « le
  login se fait via danse ». `auth::DEFAULT_CONSUMER_KEY` = consumer
  « clever-compose » du propriétaire (`4cUX75…`, base URL
  `http://127.0.0.1:7777/`) ; `DEFAULT_CONSUMER_SECRET` fourni par le propriétaire
  (capture de la console ; lecture vérifiée par un request token, les
  variantes I/l et 0/O refusées) ; l'environnement prime toujours. Retiré : `remote.rs`,
  `--api-url`/`--api-key` globaux, `login --api-url`, `Stored.api_url`,
  route `GET /api/session/login/{request_token}`, en-têtes
  `X-CC-Consumer-*`, feature `multipart` de reqwest. Le front garde
  `VITE_API_URL`, les jetons par requête et la danse ; `--api-key` reste
  pour une interface déployée à part.
- **Plus de consumer embarqué (17/09/2026)** : « retire les tokens CLI
  par défaut, on demande de fournir obligatoirement ce qu'il faut dans
  l'env » ; « clever-compose login passe par l'API à quel moment ? ».
  Les constantes `DEFAULT_CONSUMER_*` du SDK ne sont plus utilisées ;
  `Consumer::from_env()` échoue sans `CLEVER_CONSUMER_KEY`/`SECRET`, et
  avec lui `login`, `ui`, `api`, et `CLEVER_TOKEN`/`SECRET` (le consumer
  émetteur est requis). Le mode « console » (`cli-oauth`,
  `/v2/self/cli_tokens`) disparaît partout : la danse est la classique à
  trois pattes (`auth::request_token`/`access_token`, PLAINTEXT,
  helpers partagés CLI/serveur). `login` seul : callback local
  `http://127.0.0.1:<port>/callback` (`--callback-port`, 7777 par défaut
  pour partager le consumer avec `ui`), page « vous pouvez fermer »,
  jetons signables en local. `login --api-url` : la danse passe par le
  serveur (`POST /api/session/login {cli: true}` → `request_token`, le
  callback garde les jetons 10 min, `GET /api/session/login/{request_token}`
  les rend une fois) ; `Stored.api_url` marque ces jetons comme valables
  seulement avec ce serveur (`local_credentials`/`remote_credentials`
  refusent le reste, consumer vide côté poste, le serveur signe avec le
  sien). Front : seul le mode redirection reste. Vérifié : refus clairs
  sans consumer (api, login, CLEVER_TOKEN), request_token refusé avec un
  secret faux (400 « OAuth signature is invalid »), consumer inconnu via
  le serveur (404 « OAuth consumer not found ») ; la danse complète reste
  à faire par le propriétaire avec son consumer.
- **Consumer du client (17/09/2026)** : « et pour filer des consumers
  spécifiques à l'api ? » — `CLEVER_CONSUMER_KEY/SECRET` sur le serveur
  ; en relisant, un jeton OAuth 1.0 n'est valable qu'avec son consumer
  émetteur et le serveur signait avec le sien : une CLI connectée par
  `login` (consumer clever-tools) contre un serveur à consumer propre
  aurait été refusée. La CLI envoie donc aussi `X-CC-Consumer-Key`/
  `X-CC-Consumer-Secret` ; le serveur signe avec, et n'emploie le sien
  que pour des jetons sans consumer (issus de sa propre danse).
- **Jetons du client, pas de session serveur (17/09/2026)** : « faut que
  ce soit le client qui envoie ses tokens ou qu'il fasse la danse. Une
  session par utilisateur » ; « la cli doit permettre de faire la danse
  avec clever-compose login » ; « ou alors utiliser les tokens en var
  d'env » ; « prends exemple sur les clever tools » ; « un middleware qui
  vérifie que les tokens sont valides chez clever cloud » ; « un fichier
  .json peut être mis dans .config/clever-cloud » ; « si tokens invalides
  on propose de se connecter côté UI, idem côté CLI ». Fait : `auth.rs`
  (`login` = `cli_token` aléatoire, page `cli-oauth` de la console,
  polling `/v2/self/cli_tokens`, vérification `/v2/self`, fichier
  `~/.config/clever-cloud/clever-compose.json` en 0600 ; `credentials()` :
  env puis fichier ; `logout`). Serveur : plus de session partagée ;
  extracteur `Client` (`X-CC-Token`/`X-CC-Secret`, `X-CC-Api-Token`,
  `X-CC-Org`), `State::api_for` valide chez Clever Cloud avec cache 5 min,
  repli sur les identifiants du serveur pour les clients sans jetons
  (usage local conservé) ; `GET /api/session` dit `authenticated`,
  `email`, `reason`, `own_tokens` ; connexion navigateur :
  `POST /api/session/login` (`console` : URL + `cli_token`, puis
  `GET /api/session/login/{cli_token}` un tour à la fois ; `redirect` :
  callback → `#token=…&secret=…`) ; CORS autorise les nouveaux en-têtes.
  Front : jetons en sessionStorage, organisation en localStorage envoyée
  en `X-CC-Org` (plus de `POST /api/session`), « Se déconnecter », jetons
  refusés → oubliés et écran de connexion avec la raison. CLI :
  `remote.rs` envoie les jetons du poste ; `fail()` reconnaît un échec
  d'identifiants et propose `login` (lancé sur `o` dans un terminal).
  Vérifié en local : serveur sans identifiants → `authenticated:false`
  avec raison, `login` console → `{ready:false}` en attendant, jetons
  bidon → 401 « Unknown token » côté API et hint côté CLI, écran de
  connexion en headless ; la fin de la danse (dépôt des jetons par la
  console) reste à valider par le propriétaire.
- **Trois binaires et exécution distante (17/09/2026)** : demandes « un
  binaire uniquement CLI, un uniquement API et un UI+API » et « indiquer
  une URL d'API à la CLI pour de l'exécution distante ». Features cargo
  `api` (axum, tokio-stream, zip, getrandom optionnels) < `ui` (rust-embed,
  mime_guess) < `ui-bundled`, défaut `ui` ; `web` et les commandes `ui`/`api`
  sous `cfg`. `remote.rs` : client reqwest (même 0.12 qu'oauth10a, feature
  `multipart`) ; `--api-url`/`--api-key` globaux (`CC_API_URL`/`CC_API_KEY`) ;
  up/seed/export envoient le projet en tar.gz (exclusions `.git`,
  `node_modules`, `target`, `.terraform`, fichiers > 64 Mio), lisent le SSE
  et retirent le projet temporaire ; down/stop/start/ps/logs visent le
  projet par son nom (calculé comme en local) ; `ps` gagne une route SSE.
  Organisation par requête (`X-CC-Org`, extracteur `ReqOrg`, `api_of(s,
  org)`) : la session du serveur est partagée, l'en-tête évite qu'un client
  change l'organisation des autres. Clé d'API (`--api-key`, middleware
  `require_key`, exemptions `/api/info`, callback OAuth, `OPTIONS`) ;
  l'interface la demande une fois par onglet (sessionStorage) quand
  `/api/info` la déclare requise. Vérifié en local : 401 sans clé, export
  distant écrit `main.tf` et `bootstrap.sh`, `up --dry-run` distant passe
  par le serveur jusqu'à la plateforme, projet temporaire retiré ; les
  trois variantes compilent, clippy strict sur chacune.
- **Consumer de repli et connexion obligatoire (17/09/2026)** : demandes
  « renseigner des consumers en var d'env, fallback sur ceux de la CLI
  clever qui peuvent être écrits en dur » et « si la danse OAuth est
  active parce que pas de user tokens, rediriger systématiquement vers le
  login avant d'afficher autre chose ». `OauthConsumer::from_env` :
  `CLEVER_CONSUMER_KEY`/`SECRET`, sinon `clevercloud_sdk::DEFAULT_CONSUMER_*`
  (ceux de clever-tools, `builtin`). Vérifié : ce consumer refuse tout
  `oauth_callback` (« OAuth callback is invalid », 400) ; il se connecte
  donc comme `clever login` (`src/commands/login/login.command.js` de
  `~/clever-tools`) : `https://console.clever-cloud.com/cli-oauth?cli_version=&cli_token=`
  (20 octets aléatoires, base64url) puis polling de
  `GET /v2/self/cli_tokens?cli_token=` (404 jusqu'au dépôt des jetons,
  2 s × 60). `POST /api/session/login` rend `{mode: console|redirect, url}`
  ; en `console` le serveur interroge en tâche de fond et
  `GET /api/session` expose `login_pending`/`login_error`. `ui`/`api` ne
  refusent plus de démarrer sans identifiants. Front : sans session, seul
  l'écran de connexion s'affiche et démarre de lui-même (redirection, ou
  onglet console + polling de la session toutes les 2 s) ; le reste de
  l'application n'est monté qu'après (`boot`). Mise en page mobile
  (demande du même jour) : une colonne sous 900 px, menu des projets en
  tiroir plein écran, détail sous le diagramme, options défilantes ;
  vérifié en headless à 390 px.
- **Travail bloquant hors du runtime, résolveur d'images partagé,
  répertoires de travail uniques (17/09/2026)** : audit #12, P3, P4 et
  une part de S12. `reconcile::up` faisait `repo_files` (lecture du
  contexte), `git::build`/`remote_head`/`push` (libgit2 synchrone) sur le
  worker Tokio, dans `temp_dir()/clever-compose-<projet>-<service>`
  (deux `up` du même projet s'écrasaient) ; `local_list`/`local_files`
  chargeaient les compose dans le handler ; `Registry::new()` (un
  runtime multi-thread + client OCI) renaissait à chaque `compute`.
  Désormais : bloc git dans `spawn_blocking`, `unique_work_dir`
  (`clever-compose-<pid>-<n>-<quoi>`, 0700, refus si existant) pour les
  dépôts, l'exécution locale et l'export ; `project_name`/`text_files`
  dans `spawn_blocking`, `local_file_get/put` en `tokio::fs` ;
  `State::registry(confine)` : deux résolveurs partagés (poste,
  anonyme), créés à la première demande ; `docker_auths()` lu une fois
  par `up`.
- **Lectures API en parallèle borné (17/09/2026)** : audit #12, P2.
  `inspect::projects` et `discover` appelaient `get_tags` en série pour
  chaque application (toutes celles de l'organisation pour `projects`,
  rafraîchi par l'interface) ; `ps` et l'inventaire ajoutaient
  `instances`, `last_deployment`, `list_domains` en série par
  application. `inspect::each` (`futures_util::stream::buffered`,
  `CONCURRENCY` = 8, ordre conservé), `with_tags`, `app_status`
  (`join3` des trois lectures) et `status_keys` ; `discover` filtre par
  préfixe avant de lire les tags. Dépendance `futures-util` (déjà dans
  l'arbre, `alloc` seul). Le `get_tags` des homonymes dans
  `reconcile::up` reste en série (rare).
- **Base publique sur `Host` seul, connexions bornées, limiteur
  (17/09/2026)** : audit #12, S6 et S7. `public_base_for` prenait
  `X-Forwarded-Host`, qu'un client peut poser lui-même : callback OAuth
  et `return` détournables derrière un proxy laxiste. Désormais `Host`
  (validé par `same_site` en écoute locale) et `X-Forwarded-Proto`
  restreint à http/https ; avis au démarrage sans `--public-url` hors
  loopback. `pending` : `PENDING_TTL` 10 min (purge à chaque début,
  vérifié au callback), `PENDING_MAX` 1000 → 429. `Limiter` (fenêtre
  fixe par adresse, 20/min) sur `POST /api/session/login` (un request
  token chez Clever Cloud par appel) et sur la validation de jetons
  inconnus (`api_for`, un `/v2/self` par tentative) ; `Client::peer`.
  Tests : fenêtre par adresse, jetons coupés avant l'API, 429 sur login.
- **Archives bornées, extraction en flux hors du runtime (17/09/2026)** :
  audit #12, S8. Corps de 256 Mio en mémoire, chaque entrée lue en
  mémoire sans plafond de taille décompressée (zip bomb), le tout dans le
  handler async. `ARCHIVE_LIMIT` 64 Mio (corps par défaut),
  `SEED_UPLOAD_LIMIT` 1 Gio sur la seule route du dump ; `Budget`
  d'extraction : 512 Mio décompressés, 64 Mio par fichier, 20 000
  entrées, copie en flux (`take` + sonde d'un octet), fichier partiel et
  racine retirés en cas de refus ; `add_archive` dans `spawn_blocking` ;
  dump de seed écrit en flux (`tokio::fs`, `field.chunk()`). Tests :
  zip de 64 Mio + 1 refusé et nettoyé, tar de 20 001 entrées refusé.
- **Nom de projet validé, répertoires temporaires neufs (17/09/2026)** :
  audit #12, S9. `{project}` des routes plateforme entrait tel quel
  (percent-décodé par axum) dans les clés S3, `temp_dir().join(project)`
  (un chemin absolu remplace la racine) et les tags ; `Locals::remove`
  remontait les parents jusqu'à `/` faute de racine reconnue ;
  `add_archive` nommait sa racine avec `next.load` (deux dépôts
  simultanés, même répertoire). `project_param` : forme des noms Compose
  (minuscules, chiffres, `-`, `_`, 128 max), 400 sinon, sur `inventory`,
  `logs`, `ps`, `down`, `stop`, `start`, `assets`, `assets/open`.
  `fresh_temp_dir` : `clever-compose-ui-<pid>-<n>-<quoi>` en 0700,
  compteur atomique, refus si le chemin existe ; `remove` ne supprime que
  l'ancêtre `clever-compose-ui-…`, sinon rien. Tests.
- **`seed` web : plan hors du runtime (17/09/2026)** : audit #12, P1.
  `local_seed` construisait `Registry` et appelait `seed_plan` (donc
  `images.resolve` → `block_on` du runtime du registre) dans la tâche
  Tokio : panique « Cannot start a runtime from within a runtime » dès
  qu'une image n'était pas en cache, SSE jamais terminé. Comme `local_up`,
  tout le calcul passe dans `spawn_blocking`.
- **Assets authentifiés et par organisation (17/09/2026)** : audit #12,
  S4. `GET /api/platform/projects/{p}/assets` n'avait ni extracteur
  `Client` ni validation : quiconque atteignait le serveur lisait tout
  préfixe du bucket avec les identifiants Cellar du serveur ; le préfixe
  `<projet>/` mélangeait les organisations. Préfixe
  `<propriétaire>/<projet>/` (`owner_id()` de la plateforme, `X-CC-Org`
  côté web), `assets_get`/`assets_open`/`assets/sync` passent par
  `api_of` ; `project_files` écarte les fichiers déclarés `secrets.*.file`
  (le shim les matérialise depuis l'environnement). Les `.env` et les
  valeurs d'`environment` restent dans les assets et `plan.json` : le
  projet doit se recharger tel quel, et la lecture est désormais
  réservée à l'organisation. Les assets poussés avant ce changement
  (préfixe sans propriétaire) ne sont plus vus : à repousser. Test de
  route : 401 sans identifiants.
- **Projet déposé : rien du serveur dans le plan (17/09/2026)** : audit
  #12, S3. `secrets.environment` faisait lire `std::env::var` du serveur
  au `up` (`reconcile::resolve`) et une image d'un registre connu de
  `~/.docker/config.json` du serveur recevait ses `CC_DOCKER_LOGIN_*`.
  `Context::confined` (depuis `Options::confine`) : diagnostic
  `secret-environment-confined` ; `private_registries` vide ;
  `Registry::anonymous()` pour lire les manifestes sans identifiants
  (`web::api::registry(confine)`). Test dans `tests/confine.rs`.
- **Projet déposé confiné à son répertoire (17/09/2026)** : audit #12,
  S2. `compose/paths.rs` résolvait `~` et laissait passer les chemins
  absolus (comportement Compose) : depuis une archive envoyée à un
  serveur, `env_file: /proc/self/environ` ou `secrets.file:
  ~/.config/clever-cloud/clever-compose.json` revenaient dans
  `GET /api/local/{id}/plan`. `Options::confine` → `Config::confine`
  (racine) → `paths::check_confined` sur chaque chemin résolu
  (`build.context`/`dockerfile`/`additional_contexts`/`ssh`,
  `configs.file`, `secrets.file`, `env_file`, `label_file`, `include.*`,
  `develop.watch`, `extends.file`, bind `source`, volume `device`) et sur
  chaque fichier lu par le chargeur (compose, include, extends, env) ;
  diagnostic `path-outside-project` avec chemin et position. Vérification
  lexicale plus canonique (liens). Côté web, `Locals::get` dit si un
  projet est déposé (temporaire ou à propriétaire) : `compute` et
  `project_name` confinent alors ; un répertoire du poste ouvert en local
  et la CLI restent libres. `tests/confine.rs` : neuf clés, `~`, et un
  projet sain.
- **Requête du poste, anti-CSRF et anti-rebinding (17/09/2026)** : audit
  #12, S5. Le prêt des identifiants du serveur reposait sur « écoute
  loopback » seul (un reverse proxy vers `127.0.0.1` les prêtait à tous)
  et rien ne vérifiait `Host` ni `Origin` : depuis n'importe quelle page,
  `fetch(..., {method: 'POST', mode: 'no-cors'})` sans corps faisait
  passer `Option<Json<DownBody>>` à `None` (axum 0.8.9) et exécutait
  `down`/`up`/`stop` avec les identifiants du poste ; un nom DNS pointé
  vers 127.0.0.1 rendait même les requêtes JSON « same-origin ».
  `Client::from_host` (pair loopback via `ConnectInfo`, aucun en-tête de
  proxy) : `State::api_for`/`owner_of` prennent le `Client` et ne prêtent
  le repli qu'à une requête du poste ; `local_execution_allowed` s'en
  sert. Middleware `same_site` sur `/api/*` : `Host` admis (écoute,
  `localhost`/loopback sur le port, hôte de `--public-url`, hôtes
  `--cors` ; vérifié en écoute locale ou avec `--public-url`, 421 sinon) ;
  sur `POST`/`PUT`/`DELETE`/`PATCH`, `Sec-Fetch-Site: cross-site` ou un
  `Origin` ni même origine ni `--cors` → 403. Proxy Vite : `changeOrigin`
  et retrait de l'`Origin`. Tests de route : repli refusé hors du poste,
  `Host` étranger 421, `Origin` étrangère et cross-site 403, même origine
  passe.
- **Exécution locale refusée sur un serveur partagé (17/09/2026)** : audit
  de sécurité (issue #12, S1). En rendant le serveur exposable, la garde
  « réservé à l'usage local » avait été posée sur l'ouverture d'un
  répertoire du poste, pas sur l'exécution des tâches `run: local`
  (`x-clever.init: local`, `seed --local`) : un client à jetons faisait
  tourner son script sur la machine du serveur (`exec::local::run_task`,
  `sh` ou `docker run`). `web/api.rs` : `local_execution_allowed` (écoute
  locale **et** client servi par les identifiants du serveur) ; `local_up`
  refuse un plan qui porte une app `RunWhere::Local`, `local_seed` refuse
  `local: true` (403), message qui renvoie vers la tâche plateforme.
  Revue de la première version (27bba49) : une écoute loopback derrière un
  reverse proxy public passait la garde, et « pas de jetons » était
  déduit de `owner().is_none()` (id de `/v2/self`) plutôt que des jetons
  eux-mêmes. Deuxième version : `--local-exec auto|on|off`
  (`CLEVER_COMPOSE_LOCAL_EXEC`), `auto` refusant dès qu'un indice de
  publication existe (écoute non locale, `--public-url`, `--cors`,
  `--api-key` ; `local_exec_policy`), et par requête `c.tokens.is_none()`,
  pair loopback (`ConnectInfo`, `into_make_service_with_connect_info`) et
  aucun en-tête de proxy (`Forwarded`, `X-Forwarded-*`, `X-Real-IP`,
  `Via`) ; garde évaluée avant tout appel réseau. `/api/info` expose
  `local_exec` ; le front masque « exécuter depuis ce poste » quand le
  serveur la refuse ou que le client a ses jetons. Tests de route
  (`tower::ServiceExt::oneshot`, dev-dependency `tower`) : jetons, pair
  distant, loopback proxyfié, serveur publié → 403 ; depuis le poste la
  garde passe. La CLI et `ui` sur le poste sont inchangés.
- **Shim compilé par `build.rs` (16/09/2026)** : le propriétaire découvrait
  un `target/release/cc-shim` (version hôte, glibc, inutile) à côté du shim
  musl de `target/x86_64-unknown-linux-musl/shim/` compilé à la main le
  13/09 ; entre `justfile` et `build.rs`, choix « build.rs ». Le script de
  build de la CLI lance lui-même `cargo build -p cc-compose-shim --profile
  shim --target x86_64-unknown-linux-musl` dans `target/shim-build`
  (répertoire séparé : un cargo imbriqué sur le même `target` attendrait
  le verrou du build parent), sans propager `RUSTFLAGS`/`CARGO_BUILD_*`
  de l'hôte, avec `rerun-if-changed` sur les sources du shim, du modèle et
  `Cargo.lock`. Cible musl absente : avertissement et shim vide, comme
  avant. `default-members = ["crates/cc-compose"]` : `cargo build` ne
  construit plus le shim pour l'hôte. Vérifié : build depuis zéro (30 s),
  relance après `touch` d'une source du shim, binaire `static-pie`,
  clippy strict et tests.
- **Scalabilité depuis l'interface, API seule (16/09/2026)** : demandes
  « configurer la scalabilité, avec édition du compose et possibilité de le
  dl pour remplacer l'existant ou mettre à jour les assets ; si les
  ressources existent déjà on ne les recrée pas mais on applique les
  changements ; les .env doivent être supportés », puis « l'API sans l'UI
  pour déployer l'UI à part, en indiquant l'URL de l'API », « une var
  d'env côté front, sinon fallback vers default ». Fait : formulaire dans
  le détail d'une application (flavors du catalogue via `/api/flavors`,
  instances), `PUT /api/local/{id}/scaling` qui écrit `x-clever` dans le
  compose par une édition textuelle (`compose/edit.rs` : le reste du
  fichier, commentaires compris, n'est pas réécrit ; `flavor` quand
  min = max, sinon `min_flavor`/`max_flavor` ; refus du bloc en `{…}`,
  du service absent) ; « Télécharger » le fichier affiché ; « Mettre à
  jour les assets » (`POST …/assets/sync`, SSE) quand `CC_ASSETS_BUCKET`
  est posé. Le `up` retrouvait déjà les applications par nom et tags et
  appliquait `update_scaling` si différent ; les addons ne sont jamais
  recréés (changement de plan journalisé, manuel). Le `.env` était déjà
  interpolé (`compose/project.rs`) ; il est maintenant listé juste après
  le compose dans l'éditeur, et poussé avec les assets comme tout fichier
  texte. `clever-compose api [--cors ORIGINE...] [--public-url]` : mêmes
  routes sans l'interface (404 explicite hors `/api`), CORS écrit à la
  main dans un middleware axum (pré-requête `OPTIONS`, en-têtes seulement
  pour une origine listée, `Vary: Origin`) ; la danse OAuth prend
  `?return=` (chemin local, ou une origine `--cors`/la base publique,
  sinon refus) pour revenir sur l'interface déployée ailleurs.
  `--public-url` sur `ui` et `api` pour le callback derrière un proxy.
  Front : `VITE_API_URL` lue au build (`web/src/api.js`, `API_BASE`
  préfixe toutes les requêtes, SSE et lien OAuth compris) ; absente, même
  origine, c'est-à-dire le serveur qui a servi la page. Vérifié en local :
  pré-requête CORS, refus d'une origine inconnue, écriture de `x-clever`
  et relecture par le plan, `.env` interpolé.
- Redirections UDP : pas disponibles nativement pour le moment
  (propriétaire, 16/09/2026). DocumentDB : mis de côté (pas dans le provider
  Terraform). Pas de contribution externe pour le moment.
- **Règle (propriétaire, 16/09/2026) : la compatibilité Terraform suit le
  provider.** L'export s'aligne sur le provider publié (schéma embarqué,
  version dans `data/PROVENANCE.md`) ; à chaque version, régénérer le
  schéma, relire `deployment`/`networkgroups`/`dependencies`, relancer
  `terraform validate`. Ce que le provider ne sait pas faire reste un
  avertissement d'export et une PR côté provider, jamais une invention ici.
- **Revue contre les sources du provider Terraform 2.2.0 (16/09/2026,
  `~/cc-compose/terraform-provider-clevercloud`)**, demandée par le
  propriétaire. Conforme : `networkgroups[].fqdn` validé contre les suffixes
  DNS de la plateforme (`pkg/resources/ng.go`, nouveau en 2.2.0) — notre
  alias `<service>.m.<ng>.cc-ng.cloud` passe ; `deployment.commit` en
  `refs/heads/…` gardé tel quel (`resolveTargetCommit`) ; `dependencies`
  attend les ids réels d'addons (`postgresql_xxx`), ce que `.id` renvoie ;
  `container_port` = `CC_DOCKER_EXPOSED_HTTP_PORT`, jamais émis. Vérifié en
  réel (`cctfaddon`, détruit) : `terraform apply` avec addon PostgreSQL +
  `dependencies` + `repository = file://<dépôt local>` (le provider ouvre un
  dépôt local sans clone : plus besoin d'héberger les branches pour
  tester) ; le commit déployé est celui de l'exécuteur (f69cc6c57cd6).
  Corrigé chez nous : schéma embarqué passé en 2.2.0 avec
  `clevercloud_elasticsearch` (les addons ES sortent en Terraform), pin
  `~> 2.2` ; registre privé enfin câblé (`registry_url/user/password` en
  variables sensibles côté Terraform, `CC_DOCKER_LOGIN_*` posés par
  l'exécuteur depuis la configuration Docker du poste ; « privé » = le poste
  a des identifiants pour l'hôte, fixé par `plan-options.json` dans les
  goldens) ; `x-clever.private: true` était parsé et documenté mais jamais
  appliqué → refus explicite `addon-private-unsupported` en attendant (le
  provider 2.2.0 sait le faire, `networkgroups` sur les ressources addon ;
  reste à décider comment l'application joint l'addon par le groupe) ; les
  applications créées par Terraform n'ont pas de tags (pas d'attribut
  `tags` sur `clevercloud_docker`) : `ps`/`logs` les montrent quand même
  par leur nom, `down` les laisse à `terraform destroy`. Toujours absent du
  provider : le lifetime Task (`instanceLifetime`), les redirections UDP,
  les tags d'application — trois PR candidates.
- **Rabattement silencieux trouvé au passage (16/09/2026)** : un plan
  partagé (`dev`) sert la version de son cluster quelle que soit la version
  demandée (`postgres:16` sur `dev` en `par` → 15 servi, par l'exécuteur
  comme par Terraform). Le catalogue porte désormais `shared_versions` par
  zone (clusters de `/v4/addon-providers/{id}` : PostgreSQL 15 en par/mtl,
  MySQL 8.0 en par, 8.4 en mtl) ; le plan refuse (`addon-version-shared`)
  avec les deux sorties : `x-clever.version: <servie>` ou un plan dédié
  (slug et prix indiqués). Les cas synthétiques du corpus déclarent la
  version servie ; Mastodon, Plausible et Sentry (14/16 demandés) sortent
  maintenant en refus, ce qui est la vérité.
- Le test `terraform validate` isole la configuration CLI du poste
  (`~/.terraformrc` avec `dev_overrides`, build local dans
  `~/.terraform.d/plugins` qui exclut le registre) : `provider_installation
  { direct {} }` via `TF_CLI_CONFIG_FILE`.
- **Scalabilité (13/09/2026, demande du propriétaire)** : le plan porte
  désormais `min_flavor`/`max_flavor` (évolution de `Scaling`, l'exécuteur
  envoie `minFlavor`/`maxFlavor`, Terraform `smallest_flavor`/
  `biggest_flavor`, script `clever scale --min-flavor/--max-flavor`).
  `x-clever.flavor` reste le raccourci min = max ; `min_flavor`/`max_flavor`
  activent le scaling vertical, `min_instances`/`max_instances` l'horizontal
  (vérifié : « Auto scalability: enabled, Scalers: 1 to 2, Sizes: XS to
  S »). `deploy.resources.limits.cpus` compte désormais dans le choix de
  flavor (plus petite couvrant mémoire **et** vCPU ; `resources-capped` si
  aucune ne suffit). `replicas: N` seul donne N instances fixes avec
  l'avertissement `replicas-fixed` (choix explicite plutôt que défaut
  silencieux). Erreurs : `flavor` avec `min/max_flavor`, bornes inversées.
  Au passage, `cpus` est rendu en flottant comme `NanoCPUs` de compose-go
  (divergence du différentiel corrigée). Corpus : `scaling`,
  `scaling-invalid`.
- **Exécution locale des scripts d'init (13/09/2026)**, alternative demandée
  par le propriétaire (« pourquoi ne pas les lancer en local plutôt que
  dans une Clever Task ? » — « c'est bien d'avoir l'alternative, on garde
  la task par défaut »). `x-clever.init: local` sur le service substitué,
  et `seed --local`. Évolution additive du plan : champ `run` sur `App`
  (`platform`, défaut et absent du JSON, ou `local`) ; la tâche reste dans
  le plan, donc dans l'export Terraform et le script, qui n'ont pas de
  « local ». `up` ne la crée pas sur la plateforme et l'exécute à son tour
  dans l'ordre topologique (`exec/local.rs`) : variables lues sur
  `/addons/{id}/env` (comme le lien), mêmes fichiers et même runner ; avec
  Docker si le démon répond (`docker run` de l'image du plan), sinon avec le
  client du poste (`CC_INIT_DIR`, `TMPDIR`). Vérifié sur PostgreSQL : init
  via Docker, second `up` arrêté par le témoin, repli `psql` sans Docker,
  `seed --local` deux fois. Arguments pour la tâche par défaut : joignabilité
  de l'addon depuis le poste non garantie, client requis, export complet.
- Alternative notée par le propriétaire (13/09/2026), à explorer plus tard :
  DocumentDB sur PostgreSQL pour remplacer MongoDB (l'addon Mongo est figé
  en 4.0.3).
- SDK Rust : PR #81 (mai 2025, approuvée, en conflit) apporte les network
  groups ; à rebaser plutôt qu'à refaire. Bug de désérialisation de
  `v2::plan::list`.

---

## Phase 3 : exécuteur et tags

*Sortie :* atteinte le 13/09/2026 (voir État courant).

*Décidé :*

- **Identifiants propres à l'outil** : `CC_API_TOKEN` (bearer, api-bridge)
  ou `CLEVER_TOKEN` + `CLEVER_SECRET` (OAuth 1.0, `CLEVER_CONSUMER_KEY/SECRET`
  optionnels, défauts publics de clever-tools). Jamais de lecture du fichier
  de clever-tools. Pour les tests, exporter les valeurs du profil dans
  l'environnement.
- **`Platform`**, trait de l'exécuteur ; implémentation `Api` sur le client
  signé du SDK Rust (`Request::execute` pour les réponses sans corps).
  Routes appelées directement, à contribuer au SDK : `GET/POST
  /v2/organisations/{o}/applications`, `PUT .../applications/{id}` (scaling),
  `GET/PUT .../tags`, `GET .../env` (liste) et `PUT .../env` (**objet nom →
  valeur**, pas la liste : 400 sinon), `GET/PUT .../vhosts/{fqdn}`,
  `GET/POST .../tcpRedirs` et `udpRedirs`, `POST .../instances`
  (redéploiement), `GET .../deployments?limit=1` (`id` numérique, `uuid`,
  `state` WIP/OK/FAIL), `GET /v2/products/instances` (version et variante
  Docker, relues, jamais codées), `GET/POST /v4/networkgroups/...` (création
  sans corps de réponse).
- **Réconciliation sans état** : network group retrouvé par label,
  application par nom puis confirmée par le tag `compose-project=<projet>`
  (refus de toucher une application homonyme sans ce tag), scaling comparé,
  tags posés, liaison si absente, **environnement comparé à l'existant et
  remplacé seulement s'il change** (ce qui décide du redémarrage),
  redirections et domaines par présence.
- **Dépôts synthétiques avec `git2`** (gitoxide n'a pas de push) : commit
  déterministe (auteur `clever-compose`, date 0, index trié), push HTTPS
  forcé sur `master` avec `token:secret` en basic (ce que fait clever-tools
  sur `deployment.httpUrl`) ; `ls-remote` avant : même commit ⇒ push sauté.
- Déploiements dans l'ordre topologique du plan, attente du succès
  (`OK`) ou échec (`FAIL`, `CANCELLED`), 30 min max ; redéploiement
  explicite si seul l'environnement a changé ou si l'application n'a jamais
  été déployée.

*Questions ouvertes restantes :*

- Le contexte de build est entièrement lu en mémoire ; pas de
  `.dockerignore`.
- `down` (suppression par tags) n'existe pas encore.

## Phase 4 : network groups et DNS

*Sortie :* atteinte le 13/09/2026 (voir État courant). WordPress joint
MariaDB via `WORDPRESS_DB_HOST` réécrit en FQDN de membre ; aucune
modification du fichier source ; page d'installation servie.

*Décidé :*

- **Stub DNS du shim écrit le 13/09/2026**, sur un test qui l'exigeait :
  un nginx avec `proxy_pass http://app:8000` dans un fichier de
  configuration copié échouait en `[emerg] host not found in upstream
  "app"` (cas `nginx-proxy`, projet `ccnginx`). Implémentation (`shim/dns.rs`,
  `hickory-proto`) conforme au §8 : 127.0.0.153:53 en UDP, `resolv.conf`
  réécrit en place sans `search`, amont = nameservers d'origine, pas de
  cache, CNAME alias → FQDN plus les enregistrements amont, AAAA relayé,
  autres noms relayés. Piège rencontré : `Name::from_utf8` de hickory
  refuse les `_` des identifiants (`app_…`, `ng_…`), les noms se
  construisent par labels bruts. Après le stub : « bonjour depuis app »
  servi par nginx à travers le network group.
- Utilisateur numérique (`USER 65532`, images distroless sans `/bin/sh`) :
  lu directement par le shim, pas d'étape `RUN` de résolution.
- Limite constatée : un port réel imposé par la `command` et différent de
  l'`EXPOSE` de l'image (http-echo `-listen=:8000`) n'est pas décidable
  statiquement ; `x-clever.http_port` le fixe, le diagnostic
  `port-assumed-http` le signale.

---

## Phase 2 : les deux émetteurs

*Sortie :* atteinte le 13/09/2026 (voir État courant).

*Décidé :*

- **Schéma du provider 2.1.0 embarqué** (`data/terraform-provider-schema.json`,
  sortie de `terraform providers schema -json`, provenance dans
  `data/PROVENANCE.md`) et relu par l'émetteur, qui vérifie chaque attribut
  émis contre lui (`terraform-schema` en cas d'écart).
- **Terraform en JSON natif** (`main.tf.json`, option évoquée au §10) :
  aucune règle d'échappement HCL, validable par `terraform validate` ; le
  rendu HCL par templating reste possible plus tard. Variables
  `organisation`, `repository` (dépôt des branches `clever-compose/<service>`),
  et une variable sensible par secret d'environnement ; les secrets fichier
  passent par `file("${path.module}/…")`, jamais en littéral.
- `networkgroups[].fqdn` requis par le provider et non auto-référençable :
  alias `<service>.m.${ng.id}.cc-ng.cloud` posé (accepté par l'API, sans
  effet DNS aujourd'hui, résolvable si axo#3168 aboutit) ; les références
  d'environnement calculent le nom dérivé depuis les ids des autres
  ressources. `redirection` est un bloc unique TCP : l'UDP et les tâches
  (pas d'attribut Clever Task) produisent un avertissement.
- **Table DNS restreinte aux services référencés** (`depends_on`, hôtes de
  l'environnement) au lieu de tous les voisins de réseau : c'est ce qui rend
  le graphe acyclique et l'apply Terraform possible en une passe (réponse
  définitive à la question 2 de la phase 0). Un nom codé en dur dans une
  image se déclare par `depends_on` ; référence mutuelle sans `depends_on` →
  `mutual-reference`.
- **Script clever-tools** : bash `set -euo pipefail`, `CC_ORG` requis,
  groupes → apps → liaisons → variables (FQDN calculés
  `${APP_x}.m.${NG_y}.cc-ng.cloud`) → redirections (`clever tcp-redirs add`,
  UDP en commentaire faute de commande) → domaines → déploiements depuis
  `repos/<service>`. Secrets : `$(cat <fichier>)` ou `${VAR:?}`.
- **Dépôt synthétique** (`plan/sources.rs`, partagé avec l'exécuteur) :
  `Dockerfile` (`FROM image`) ou `Dockerfile.cc` (Dockerfile de l'utilisateur
  prolongé, `CC_DOCKERFILE`), `cc-shim.json`, `cc-shim` embarqué par
  `build.rs` (`CC_SHIM_BIN` / `CC_SHIM_PATH` en développement), fichiers
  copiés `cc-files/<i>`, résolution de `USER` en uid/gid par une étape `RUN`
  avec la libc de l'image. Contexte de build copié sans `.git`.
- Shim : abandon des privilèges entièrement dans `pre_exec` (setgroups,
  setgid, setuid), parce que `Command::uid/gid` s'appliquent avant les
  `pre_exec` et `CommandExt::groups` est instable. Vérifié : un service
  `USER node` tourne en `node` avec ses groupes.

*Questions ouvertes restantes :*

- Rendu HCL : fait le 13/09/2026 (`emit/hcl.rs`), depuis la même structure
  JSON, blocs contre attributs d'après le schéma, `type = string` nu,
  expression nue pour une interpolation unique (`var.x`, `file(...)`).
  `export` écrit `main.tf` ; le JSON reste la forme validée en test, et le
  HCL rendu est validé à son tour (`export/hcl/`).
- `deployment.repository` : **fait le 13/09/2026**. `export --branches`
  écrit les branches `clever-compose/<service>` dans le dépôt git du projet
  (blobs et arbre via `git2`, commit racine déterministe, aucun identifiant),
  à pousser par `git push origin 'refs/heads/clever-compose/*'` ;
  `var.repository` prend par défaut l'URL de `origin` convertie en HTTPS.
  Les commits sont identiques à ceux que l'exécuteur pousse.
- **Équivalence des backends vérifiée** (§11) sur `ccnginxtf` (Test
  SBRUNAT, dépôt jetable `github.com/sebartyr/cc-compose-tftest` à
  supprimer au ménage, état Terraform dans le scratchpad) : `terraform
  apply` en une passe crée le groupe, les deux applications avec le même
  environnement, les membres (alias `<service>.m.<ng>.cc-ng.cloud`), et
  déploie le même commit (`49e9bb6caf42`) que l'exécuteur ; le front sert
  « bonjour depuis app ». Le provider a lu `CLEVER_TOKEN`/`CLEVER_SECRET`
  et `TF_VAR_organisation`.

---

## Phase 1 : parsing, plan et diagnostics

*Sortie :* atteinte le 13/09/2026. Différentiel : 15 cas identiques ou
refusés à l'identique. Politique des clés relue contre le tableau du §6 :
chaque ligne a sa traduction ou son refus (`plan/policy.rs`,
`plan/mod.rs`). Ajouts de dernière relecture : `healthcheck-strict`
(2xx exigé avec `CC_HEALTH_CHECK_PATH`, source rubydeployer) et
`proxy-resolver` (§7, nginx/haproxy/traefik/openresty).

*Décidé :*

- **Pas de crate `compose_spec`** (écart au cadrage §9, assumé) : elle
  date d'octobre 2024, dépend de `serde_yaml` archivé, et ne résout pas le
  problème, puisque dans `compose-go` la fusion des overrides, `extends`,
  `include` et la canonicalisation opèrent sur l'arbre brut avant tout
  typage. Ce travail est porté sur notre arbre brut (`Node` avec positions),
  et le typage vient en dernier. Référence : les sources de compose-go
  `v2.15.1-0.20260908103050` (celle de Docker Compose 5.5.1), lues dans
  `~/go/pkg/mod`.
- **Parseur YAML** : `saphyr` (YAML 1.2, `early_parse(false)` pour obtenir
  les scalaires bruts avec leur style), avec **notre propre résolution des
  scalaires**, portée de `go.yaml.in/yaml/v4` (`internal/libyaml/resolver.go`),
  qui n'est ni 1.1 ni 1.2 : `yes`/`no` restent des chaînes, pas de
  sexagésimal, mais octal à zéro initial (`0777` → 511, `09` → 9.0 par le
  motif flottant), underscores dans les nombres, horodatages sur scalaires
  non quotés. Table vérifiée contre l'oracle
  (`tests/corpus/yaml-resolution`). Clés de fusion `<<` faites par nous.
- **`saphyr-parser` vendorisé** (`vendor/`, patch d'une ligne) : libyaml ne
  vérifie pas l'indentation en contexte flow, saphyr si ; le compose de
  Sentry (crochet fermant aligné sur sa clé) est refusé sans ce patch. À
  proposer en amont.
- **Chaîne portée de compose-go**, dans son ordre : clés en chaînes,
  interpolation (six opérateurs, `$$`, défaut substitué, conversions de type
  par chemin), `include` (répertoire et `.env` propres), `!reset`/`!override`
  appliqués à l'accumulé, `extends` (base canonicalisée, chemins résolus
  relativement à son fichier, cache, cycles), fusion par chemin
  (`override/merge.go`), unicité (`uncity.go`), schéma JSON, canonicalisation
  (`transform/*`), `OmitEmpty` (seulement `services.*.dns`), valeurs par
  défaut, chemins relatifs, `Normalize`, décodeurs du modèle typé
  (`MappingWithEquals`, `ShellCommand` via un port de go-shellwords,
  `StringOrNumberList`, `HostsList`, `UnitBytes`, `Duration`), fusion des
  `env_file`, profils, élagage des ressources inutilisées, `omitempty` Go
  (dont `OptOut` pour `create_host_path` et `required` sans omitempty).
- **Schéma JSON** embarqué depuis compose-go (`schema/PROVENANCE.md`),
  validé après la fusion de chaque fichier : clé inconnue ou type invalide
  produisent une erreur avec chemin et position.
- **Test différentiel** : `tests/corpus/<cas>/compose.yaml` (+ `.env`,
  overrides, includes) ; `expected.yaml` = sortie de `docker compose config`
  figée, régénérée par `CC_REGEN_ORACLE=1 cargo test`. Comparaison en JSON
  après une seule règle de forme : les `$$` que `config` produit en sortie
  sont ramenés à `$`. **Treize cas identiques à l'oracle** : WordPress,
  Gitea, Nextcloud, Plausible, Immich, Mastodon, Sentry, la sonde YAML, et
  cinq synthétiques (règles de fusion des overrides, `extends`, `include`,
  formes d'interpolation, profils).
- Précédence du nom de projet : `-p`, `COMPOSE_PROJECT_NAME` (shell ou
  `.env`), `name:`, répertoire.

*Invalidé :*

- L'hypothèse du cadrage que `serde_yaml_ng`/`serde_norway`/`saphyr`
  suffiraient tels quels : la résolution des scalaires doit être la nôtre.
- L'hypothèse implicite qu'un parseur YAML 1.2 strict accepte les fichiers
  réels : Sentry demande la tolérance de libyaml.

- **Plan intermédiaire, proposé le 13/09/2026** dans
  `crates/cc-compose-model/src/plan.rs`, **à valider** (§13) : ressources
  `App` (service ou tâche), `Addon`, `NetworkGroup`, `Membership`,
  `TcpRedirection`, `Domain` ; identifiants symboliques (`app/web`,
  `member/web@default`) ; valeurs d'environnement en `Template` de segments
  littéraux et de références (`MemberFqdn`, `AddonEnv`) ; intrinsèque
  (`ImageIntrinsics`, `ShimConfig`) séparé du topologique (env, membres) ;
  `resources` dans l'ordre topologique ; empreinte de la spécification sur
  des chemins relatifs au projet.
- **Planificateur** (`crates/cc-compose/src/plan/`) : modèle typé du
  sous-ensemble consommé ; politique clé par clé (`policy.rs`) où chaque clé
  est traduite, ignorée avec avertissement, ou refusée, avec chemin et
  position ; `x-clever` (`region`, `flavor`, `min/max_instances`, `task`,
  `http_port`, `protocol`, `domains`, `ephemeral`, `plan`, `private`) ;
  flavors lues sur `/v2/products/instances` et embarquées
  (`data/PROVENANCE.md`) ; configuration des images par fixture (`images.json`)
  ou par lecture du Dockerfile de l'utilisateur, le lecteur de registre
  restant à écrire ; réécriture des noms de services dans l'environnement
  (hôte d'URL, `host:port`, valeur entière seulement si la clé évoque un hôte
  : `HOST`, `SERVER`, `ADDR`, `ENDPOINT`) ; `CC_COMPOSE_DNS_MAP` ; port
  plateforme deviné (ports non HTTP connus → répondeur de santé, sinon
  relais) avec avertissement ; tâches par `service_completed_successfully`
  ou `x-clever.task` ; volumes nommés refusés sans `--allow-ephemeral`.
- **Golden files** : `plan.json` et `plan.diagnostics.txt` par cas doté
  d'une fixture d'images (`CC_REGEN_GOLDEN=1`), sur WordPress et un cas de
  refus synthétique.

- **Redirections TCP/UDP** (13/09/2026, remarque du propriétaire) : un port
  publié non HTTP devient une ressource `Redirection` (namespace `default`
  ou `cleverapps` via `x-clever.tcp_namespace`, port externe attribué par la
  plateforme, donc jamais dans le plan) ; le shim relaie 4040 vers le port
  réel (`ShimConfig.tcp`). Un seul port TCP relayable par application (une
  seule entrée 4040) : le deuxième est une erreur. Doc :
  `content/doc/develop/common-configuration/tcp-redirections.md`. cc-api
  expose `tcpRedirs` et `udpRedirs`.
- **SDK Rust** (`~/cc-compose/clevercloud-sdk-rust`, 1.0.1) : couvre
  `v2` addons, plans, `myself`, et `v4` addon providers, functions,
  products. Manquent pour l'exécuteur : applications (création, env,
  déploiements, scaling), network groups, redirections TCP/UDP, domaines.
  À contribuer par PR au fil de l'eau, derrière un trait (§2).

- **Secrets** (13/09/2026) : un secret Compose (`file:` ou
  `environment:` ; `external` refusé) devient une variable
  `CC_COMPOSE_SECRET_<NOM>` de l'application, référence `Reference::Secret`
  dans le plan (jamais la valeur), et le shim l'écrit dans le fichier cible
  (`/run/secrets/<nom>` par défaut, mode 0444, `uid`/`gid` numériques) puis
  la retire de l'environnement de l'enfant. Testé dans un conteneur.
  Évolution du plan : `App.secrets`, `SecretBinding`, `SecretSource`,
  `Reference::Secret` ; `ShimConfig.secrets`.
- **Cohérence** portée de `checkConsistency` (références non déclarées,
  clés exclusives, valeurs contradictoires) ; le harnais différentiel
  compare aussi les refus (`expected.error` : l'oracle refuse, nous aussi).

*Questions ouvertes restantes :*

- Validation du plan intermédiaire par le propriétaire du projet.
- **UDP** : d'après le propriétaire (13/09/2026), « TCP config = UDP
  config » : l'UDP entre aussi par 4040. Relais UDP implémenté dans le shim
  sur cette base. Lu dans `~/supernova` : les ports standard de la VM
  (8080, 10050, 22, 4040 `checkPort`) sont NATés en TCP par défaut, une
  redirection UDP passe par `assignExtraPort(uuid, 'udp', …)` ; et
  l'algorithme cc-api de `udpRedirs` a sa publication `hayxorp` en
  commentaire. À confirmer par un test réel en phase 3.
- **Contrôle de santé, précisé par `~/rubydeployer`** (`util/health_check.rb`,
  `scripts/deploy-docker.sh`) : `CC_HEALTH_CHECK_PATH` exige un code 2xx
  (`200...300`), le défaut `/` accepte `200...500` ; ~4 min d'attente du
  port puis ~17 min de tentatives ; en port mapping, `-p 8080:<http>
  -p 4040:<tcp>` sans `/udp`. Le répondeur de santé du shim renvoie 200,
  donc compatible dans les deux cas.
- Bind mounts de répertoires : refusés (phase 5 pour les données, à
  discuter pour les répertoires de configuration).
- Les messages du validateur de schéma sont en anglais (crate `jsonschema`).

---

## Phase 0 : validation des hypothèses conteneur

*Décidé :*

- **Le port 8080 est toujours servi par le shim en HTTP**, sous trois formes
  figées dans `cc-shim.json` (`PlatformPort`) : `image` (l'image écoute
  elle-même sur 8080), `forward` (service HTTP sur un autre port, relais TCP),
  `health` (service non HTTP ou worker, répondeur `200 OK`, conditionné à la
  joignabilité du port réel via `ready_port` s'il y en a un). Le port n'est
  ouvert qu'une fois le port réel joignable, donc « déploiement réussi » vaut
  « service prêt ». Les services non HTTP se joignent sur leur port réel dans
  le groupe.
- Sémantique PID 1 validée en local : SIGTERM relayé (arrêt propre de postgres
  en 0,5 s, code 0), enfant tué par SIGKILL propagé en 137, `waitpid(-1)` pour
  les petits-enfants.
- Abandon de privilèges seulement si l'image déclare `USER` ; identifiants
  numériques résolus au build dans `/etc/cc-shim.ids` (non exercé en phase 0,
  postgres tournant en root et utilisant gosu).
- Le shim devra **expanser les variables** (`${VAR}`) au démarrage (question 7).
- Le shim devra réécrire `resolv.conf` **en place** (tronquer puis écrire),
  jamais par renommage (question 5).
- Le nom d'un membre est déterministe : `<app_id>.m.<ng_id>.cc-ng.cloud`. Il
  est donc connu dès la création de l'app et du groupe, avant toute liaison,
  ce qui simplifie l'ordonnancement du `up` (§7) : la table DNS peut être
  posée avant la liaison.
- Le flux `clever ng` de la CLI : `clever ng create <label>`,
  `clever ng link <app_id> <ng>`, `clever ng get <app_id> -F json` renvoie
  `domainName`. Routes API v4 : `/v4/networkgroups/organisations/<org>/networkgroups/<ng>/members[/<id>]`
  (GET, POST `{id, kind, domainName}`, DELETE).

*Invalidé :*

- L'hypothèse d'un relais TCP brut suffisant pour 8080 : le déployeur parle
  HTTP, un relais vers postgres le fait planter.
- L'hypothèse qu'un worker peut ne rien écouter.
- L'espoir d'un alias DNS supprimant le stub (§4.1) : le champ existe, le DNS
  l'ignore.
- `dnsSanitizedLabel` du groupe : ne correspond à aucun nom servi
  (`<app>.m.<label>.cc-ng.cloud` et `<app>.<label>.cc-ng.cloud` → SERVFAIL).

*Questions ouvertes restantes :*

- Le `domainName` ignoré par le DNS : **expliqué par les sources d'Axo**
  (`~/axo/crates/api/networkgroup`, lu le 13/09/2026). `dns_handler.rs` ne
  répond qu'à trois formes dérivées des identifiants
  (`<memberId>.m.<ngId>`, `<hostname>.p.<ngId>`,
  `<hostname>.<memberId>.m.<ngId>`), reprise verbatim d'OVD
  (`TypedDnsHandlerActor`), et ne consulte jamais `member.domain_name`. Le
  champ est validé (`validate_member_domain_name` : forme DNS et suffixe
  configuré), stocké, exposé dans les vues membre et peer, et n'a aucun autre
  usage. Le commentaire de la validation (« otherwise it would be registered
  under a domain the handler never resolves ») montre que l'intention est
  bien que le nom résolve. Héritage d'OVD, pas un choix récent.

  **Historique retrouvé (Slack et GitLab, 13/09/2026).** Fil
  `#tech-networkgroups` du 13/08/2026 (miton18 ↔ kannar) : le champ
  résolvait autrefois par distribution dans `/etc/hosts` des VMs, aujourd'hui
  vide. kannar : « la contrainte de suffixe c'est uniquement `.cc-ng.cloud`,
  tout ce qu'il y a avant c'est ok, mais ça doit être unique » (consensus ZK
  sur l'attribution). Décision : nom complet fourni, suffixe validé. Il n'y a
  **aucune politique contre la personnalisation**, au contraire. Issues :
  - ovd#861 (suffixe en dur côté Otoroshi) et ovd#862 (validation du suffixe)
    : faites (`de297be4a`, route `/networkgroup/configuration`).
  - **ovd#863 « networkgroup DNS: actually resolve a member's domainName (the
    field is functionally dead today) » : ouverte, jamais faite**, ni dans OVD
    ni dans Axo. Le module NG a depuis été retiré d'OVD (`0682794e9`), donc
    elle est orpheline.
  - axo#3168 (blackyoup, 01/09) : suite côté Axo, angle validation
    seulement, sans lien vers ovd#863.
  - Fil `#support-interne` du 28/08 (Mustcompliance) : `fqdn` requis dans le
    provider bloque des clients ; David Legrand : « suffit de rendre le champ
    optionnel côté TF, le domaine est désormais attribué automatiquement ».

  Action retenue : commenter axo#3168 en la reliant à ovd#863, avec le cas
  d'usage clever-compose et la proposition d'implémentation (matcher
  `member.domain_name` dans `dns_handler::answer` après le lookup du cache,
  répondre les peers SERVER comme la forme `m`, resserrer la validation à
  `<ngId><suffix>`, cas dans `tests/dns_parity.rs`). **Posté le 13/09/2026 :**
  https://gitlab.corp.clever-cloud.com/clever-cloud/axo/-/work_items/3168#note_852711

  Si ce correctif arrive, le stub DNS du shim disparaît au profit d'un
  `search m.<ng_id>.cc-ng.cloud` dans `resolv.conf`, avec des membres nommés
  `<service>.m.<ng_id>.cc-ng.cloud`. En attendant, l'architecture du cadrage
  (réécriture statique + stub) reste la référence.
- Terraform : traitement des références mutuelles entre services (cycle) dans
  l'export. Candidats : diagnostic explicite, ou apply en deux passes limité à
  ce cas.
- Comportement du déployeur sur un déploiement long (`ready_port` qui tarde) :
  quel délai avant l'échec ? Constaté 4 min sans écoute ; à mesurer avec une
  écoute tardive.
- Le contrôle de santé accepte 200 à 500 : `CC_HEALTH_CHECK_PATH` change-t-il
  cette plage ? À vérifier avant de mapper `healthcheck`.
