# clever-compose : prompt de cadrage

Document racine du projet. À lire intégralement au début de chaque session.
Les phases se lisent dans l'ordre, chacune a un critère de sortie explicite.
Aucune phase ne démarre avant que la précédente ait passé son critère.

---

## 1. Objectif

Un outil qui prend un `docker compose` et le fait tourner sur Clever Cloud,
chaque service devenant une application Docker distincte. Deux promesses :

1. **Transparence.** `clever-compose up` déploie sans que l'utilisateur ait à
   réécrire son compose ni à comprendre les spécificités de la plateforme.
2. **Sortie réversible.** L'outil sait émettre la traduction en Terraform et en
   script clever-tools, pour que l'utilisateur puisse reprendre la main.

Non-objectif : reproduire toute la spécification Compose. Une bonne part des
clés doit être refusée avec un diagnostic clair plutôt qu'approximée.

---

## 2. Architecture : un compilateur, pas un runtime

Le compose est parsé, normalisé, puis traduit en un **plan cible** (une
représentation intermédiaire typée). Ce plan a trois backends :

- un **exécuteur** qui appelle l'API Clever (mode transparent) ;
- un **émetteur HCL** pour le provider Terraform ;
- un **émetteur shell** pour clever-tools.

Conséquence structurante : l'export Terraform n'est pas une fonctionnalité à
part, c'est une sérialisation de ce que l'exécuteur fait déjà. L'équivalence est
testable : appliquer via l'exécuteur ou via `terraform apply` doit produire le
même état.

Kompose (Compose vers Kubernetes) est la référence à consulter pour les choix de
mapping et la forme des diagnostics.

### Langage et découpage

Rust, workspace unique, trois crates :

- `cc-compose-model` : plan intermédiaire, config du shim, types partagés.
  Dépendances volontairement frugales, car le shim en hérite. Si l'arbre
  grossit, scinder en `model-core` (ce que le shim lit) et `model-plan`.
- `cc-compose-shim` : binaire statique musl, `no default features`,
  `panic = "abort"`, LTO en release.
- `cc-compose` : parsing, plan, exécuteur, émetteurs, CLI.

Le shim compilé est embarqué dans la CLI via `include_bytes!` et écrit
directement dans le dépôt synthétique. Pas d'image publique intermédiaire, donc
pas de dépendance à un registre tiers au build, et cohérence de version garantie
par construction. Prévoir un `cargo xtask` (ou un flag de développement) pour
charger un shim depuis le disque, la compilation croisée vers
`x86_64-unknown-linux-musl` étant pénible en local.

Architecture figée à amd64 pour l'instant. Si de l'arm apparaît, embarquer les
deux binaires et choisir selon le manifeste lu sur le registre.

### Accès API

Utiliser le SDK Rust Clever Cloud, maintenu en interne, donc les routes
manquantes se contribuent par PR. Pour ne pas dépendre du rythme de merge,
isoler tous les appels derrière un trait dans l'exécuteur, avec une
implémentation SDK quand la route existe et un appel `reqwest` direct sinon. Les
PR remontent au fil de l'eau, l'implémentation se remplace sans toucher au
reste.

Ne pas concevoir une abstraction SDK taillée pour le seul besoin de ce projet.
Une bonne route SDK se dessine après deux ou trois usages.

---

## 3. Contraintes plateforme (vérifiées en documentation)

Ces points sont issus de la documentation Clever Cloud et corrigent des
intuitions fausses. Ils ne se rediscutent pas sans nouvelle source.

**Mode réseau.** Avec les ports par défaut (`CC_DOCKER_EXPOSED_HTTP_PORT=8080`,
`CC_DOCKER_EXPOSED_TCP_PORT=4040`), le conteneur tourne en `--net host`. Dès
qu'un port est personnalisé, Docker bascule en port mapping et ne publie que ces
deux ports, ce qui casse la joignabilité sur les autres ports dans le network
group. **Règle absolue : ne jamais définir `CC_DOCKER_EXPOSED_HTTP_PORT`.** Une
image qui écoute ailleurs est relayée par le shim. `CC_MOUNT_DOCKER_SOCKET=true`
produit le même basculement, donc même interdiction.

**Volumes.** L'accès aux FS Buckets n'est pas supporté sur le runtime Docker. Un
service Docker n'a pas de volume persistant. Un service stateful devient un
addon managé, ou reste éphémère sur choix explicite.

**Build args.** Toutes les variables d'environnement de l'application sont
passées en `--build-arg` pendant le build. Les `ARG` du Dockerfile sont donc
alimentés sans effort particulier.

**Tâches.** Une application Docker peut être configurée en Clever Task : le
conteneur exécute sa commande puis s'arrête. C'est l'équivalent direct des
services one-shot de Compose (migrations avec
`depends_on: condition: service_completed_successfully`).

**Network groups.** Overlay WireGuard. Un membre (application ou addon) est
joignable sur n'importe quel port sous
`<memberID>.m.<ngID>.cc-ng.cloud`. Chaque instance est un peer avec sa propre IP
dans le CIDR du groupe. Le rattachement des addons et des peers externes est
annoncé comme non pleinement supporté en bêta.

**Hooks.** `CC_PRE_BUILD_HOOK`, `CC_POST_BUILD_HOOK` et `CC_PRE_RUN_HOOK`
s'exécutent sur l'hôte. `CC_RUN_SUCCEEDED_HOOK` s'exécute dans le conteneur mais
après démarrage réussi, donc trop tard pour préparer l'environnement. Aucun
point d'injection utilisable : d'où le shim.

---

## 4. Hypothèses à valider avant de coder la couche réseau

Ces questions conditionnent le volume de travail bien plus que n'importe quel
choix technique. Les traiter en priorité, en parallèle de la phase 0.

1. **Alias de nom.** Le bloc `networkgroups` du provider Terraform déclare
   `fqdn` comme requis, décrit comme le nom qui résout vers les instances dans
   le groupe. Si le nom est réellement imposé par la plateforme, ce champ ne peut
   pas être une entrée. Trois lectures : documentation générée trompeuse, champ
   ignoré, ou mécanisme d'alias exposé par l'API mais absent de `clever ng`.
   **Si un alias est possible, tout le stub DNS disparaît** au profit d'un
   `search` dans `resolv.conf`.
2. **Lisibilité du FQDN en Terraform.** L'environnement de `api` dépend du FQDN
   de `db`. Pour un `apply` en une passe, ce FQDN doit être un attribut lisible
   d'une ressource ou d'une data source. Sinon l'export devient un apply en deux
   étapes, ce qui abîme sérieusement la promesse de réversibilité.
3. **TTL et enregistrements** de `cc-ng.cloud` : les A renvoient-ils toutes les
   IPs des peers, avec quel TTL ?
4. **Liaison à chaud** : une application liée après le démarrage de ses
   instances doit-elle redémarrer pour rejoindre le groupe ?
5. **Résolution en mode host** : `cc-ng.cloud` résout-il depuis un conteneur en
   `--net host`, et `/etc/resolv.conf` y est-il modifiable depuis l'intérieur ?
6. **Workers sans port** : que considère le déploiement comme un succès pour un
   conteneur qui n'écoute sur rien ?
7. **Expansion des variables d'environnement.** La plateforme résout-elle une
   variable qui en référence une autre (`DATABASE_URL=...${POSTGRESQL_ADDON_HOST}...`) ?
   Si oui, l'expansion par le shim décrite en 6.2 est inutile. Si non, elle
   devient le mécanisme qui garde les secrets hors de la configuration et hors
   de l'export Terraform.

---

## 5. Le plan intermédiaire

C'est le contrat entre toutes les phases. Un enum de ressources cibles, avec des
**références symboliques** entre elles plutôt que des chaînes résolues. Les
backends restent alors triviaux et les tests se font en golden files.

Ressources : application Docker, addon managé, network group, appartenance,
redirection TCP, domaine, tâche.

Séparer explicitement dans le plan ce qui est **intrinsèque à l'image**
(entrypoint et cmd d'origine, uid, gid, groupes supplémentaires, port réel) de
ce qui est **topologique** (table DNS, secrets, identifiants d'addon). Voir la
section 7 : cette séparation est ce qui rend `up` idempotent.

---

## 6. Mapping Compose vers Clever Cloud

| Clé Compose | Traduction | Remarque |
|---|---|---|
| `image:` | dépôt synthétique avec Dockerfile `FROM image` | registres privés via `CC_DOCKER_LOGIN_*` |
| `build:` | push du contexte, `CC_DOCKERFILE` si hors racine | |
| `ports:` | exposition publique uniquement | jamais de changement du port exposé, voir section 3 |
| `expose:` | sans objet | tous les ports sont joignables dans le groupe |
| `environment`, `env_file` | variables de l'app | après réécriture des noms de services |
| `depends_on` | ordre topologique des déploiements | `service_healthy` = attendre le succès du déploiement |
| `healthcheck` | `CC_HEALTH_CHECK_PATH` si HTTP | sinon ignoré avec avertissement |
| `deploy.replicas`, `resources` | instances et flavor la plus proche en mémoire | |
| volumes nommés | addon ou refus | pas de FS Bucket sur Docker |
| bind mounts de fichiers | `COPY` dans le Dockerfile généré | config figée dans l'image |
| `secrets` | variable d'env plus matérialisation par le shim | évite le secret dans une couche |
| services one-shot | Clever Task | |
| `privileged`, `cap_add`, `devices`, `network_mode: host`, `sysctls` | refus avec diagnostic | |

Surcharges spécifiques via une extension `x-clever` au niveau service ou projet
(flavor, région, plan d'addon, domaine, arbitrage addon ou conteneur,
`private: true` pour rattacher un addon au groupe). Les clés `x-*` sont ignorées
par la spécification, donc le fichier reste utilisable en local sans
modification.

### Substitution par les addons

C'est la partie à plus forte valeur produit : faire tourner un service stateful
du compose sur le PaaS managé plutôt qu'en conteneur éphémère. C'est aussi la
plus riche en pièges, d'où le développement qui suit.

Fournisseurs pertinents : `postgresql-addon`, `mysql-addon`, `mongodb-addon`,
`redis-addon`, `es-addon` (Elasticsearch, avec Kibana et APM en options),
`cellar-addon` pour du S3, et `config-provider` qui expose de la configuration
aux applications via variables d'environnement. Ce dernier est un bon support
pour l'environnement partagé du projet compose, au lieu de dupliquer les
variables communes sur chaque application.

Comportement par défaut : proposer l'addon managé quand l'image est reconnue, et
refuser la version conteneurisée sans `--allow-ephemeral` explicite. Mais la
substitution est facturée, donc elle s'annonce dans `plan` avec son coût mensuel
et se confirme. Un postgres qui sert de fixture de test jetable ne doit pas
devenir un addon par surprise.

#### 6.1 Reconnaissance

La détection de `postgres:16` est triviale, ce qui l'entoure ne l'est pas.

- **Variantes.** `bitnami/postgresql`, `postgis/postgis`, `pgvector/pgvector`,
  `timescale/timescaledb` sont des postgres, avec des attentes et surtout des
  noms de variables différents (Bitnami utilise `POSTGRESQL_PASSWORD` et non
  `POSTGRES_PASSWORD`). La table de reconnaissance porte donc, par entrée, le
  fournisseur cible **et** le dialecte de variables du service source.
- **Images dérivées.** Un service en `build:` avec un `FROM postgres` n'est pas
  substituable : il contient probablement des extensions ou des scripts. Ne
  jamais substituer une image construite.
- **Versions.** Le tag peut être `16`, `16.4`, `16-alpine`, `latest`, ou absent.
  Mapper vers un plan portant la version demandée. PostgreSQL 18 est la version
  par défaut de la plateforme depuis le 15 septembre. Une version demandée mais
  indisponible produit un diagnostic, jamais un rabattement silencieux.

#### 6.2 Câblage des identifiants

L'addon choisit ses propres identifiants, donc ceux déclarés dans le service
compose deviennent caducs. Le lien injecte des variables normalisées dans
l'application : `POSTGRESQL_ADDON_HOST`, `_PORT`, `_DB`, `_USER`, `_PASSWORD`,
`_URI`, et les équivalents `MYSQL_ADDON_*`.

Trois cas dans l'environnement des consommateurs :

1. **URL complète** (`DATABASE_URL=postgres://user:pass@db:5432/app`). Se parse,
   et si l'hôte est un service substitué, se reconstruit intégralement depuis
   les attributs de l'addon. Cas propre.
2. **Variables discrètes** (`DB_HOST`, `DB_USER`, `DB_PASSWORD`). Corrélation
   par **égalité de valeur** avec ce que déclare le service source. Ce que
   l'heuristique ne reconnaît pas sort en avertissement, jamais deviné.
3. **Format non standard.** La documentation Clever le signale pour Docs : il ne
   faut pas connecter l'addon Redis, parce que l'application attend un format
   d'URI différent de celui fourni par la plateforme, et il faut injecter
   `REDIS_URL="redis://default:<password>@<host>:<port>"` à la main. Le lien
   seul ne suffit donc pas : le générateur produit les variables **au format
   attendu par l'application**.

**Expansion par le shim.** Plutôt que de figer les identifiants résolus dans
l'environnement de l'application au moment du plan, poser la forme non résolue
(`DATABASE_URL=postgres://${POSTGRESQL_ADDON_USER}:${POSTGRESQL_ADDON_PASSWORD}@${POSTGRESQL_ADDON_HOST}:${POSTGRESQL_ADDON_PORT}/${POSTGRESQL_ADDON_DB}`)
et laisser le shim l'expanser au démarrage depuis les variables injectées par le
lien. Trois bénéfices : aucun secret dupliqué dans la configuration, une
rotation d'identifiants effective au simple redémarrage, et un export Terraform
sans littéral secret. À vérifier d'abord : si la plateforme expanse déjà les
variables entre elles, le shim n'a rien à faire (voir section 4, point 7).

#### 6.3 Scripts d'initialisation et reprise de données

Un montage vers `/docker-entrypoint-initdb.d` est courant dans les composes
réels. Avec un addon, ces scripts ne s'exécutent jamais et l'application démarre
sur une base vide.

Traitement : une Clever Task qui exécute les scripts contre l'addon avant le
premier déploiement des consommateurs. L'idempotence doit être émulée, l'image
officielle ne rejouant les scripts que si le répertoire de données est vide :
utiliser une table témoin portant le hash des scripts.

Même mécanique pour la migration depuis un volume de développement : une
commande `clever-compose seed` qui charge un dump local dans l'addon.

#### 6.4 Falaises sémantiques

Ce sont elles qui font qu'un compose correctement traduit ne fonctionne pas.
Chacune doit produire un diagnostic nommé.

- **Pas de superutilisateur.** Les `CREATE EXTENSION` non autorisées échouent,
  et les applications qui créent leur propre base au démarrage aussi.
- **Une base par addon.** Beaucoup de composes servent plusieurs bases depuis
  une seule instance, parfois via `POSTGRES_MULTIPLE_DATABASES`. Soit un addon
  par base, soit un refus.
- **Arguments de tuning.** `command: postgres -c max_connections=200` n'a pas
  d'équivalent. Refus explicite.
- **Connexions.** Piège opérationnel principal : un pool applicatif multiplié
  par le nombre d'instances dépasse vite la limite d'un petit plan. Avertir dès
  qu'un `deploy.replicas` élevé rencontre un plan modeste.
- **Latence.** Un proxy sert toutes les bases PostgreSQL dédiées et peut ajouter
  de la latence. Un hostname et un port directs se génèrent depuis le tableau de
  bord de l'addon, mais ne survivent pas à une migration de base et doivent être
  régénérés manuellement. À proposer, pas à activer par défaut.
- **Réseau.** Les addons sont joignables par leur hôte avec leurs identifiants,
  hors network group. La substitution **retire** donc un membre du groupe et une
  entrée de la table DNS du shim, elle n'en ajoute pas.

---

## 7. Réseau et résolution de noms

Un network group par réseau Compose. `<projet>_default` donne un groupe, un
service présent sur `frontend` et `backend` devient membre des deux (le bloc
`networkgroups` est un ensemble). À l'intérieur d'un groupe, tous les peers
communiquent sur n'importe quel port : même sémantique plate qu'un réseau
Compose, la segmentation se faisant entre groupes.

**Le nom du membre est imposé mais récupérable via l'API après liaison.** Le
raccourci `search` seul ne suffit donc pas : l'utilisateur écrit `db`, le nom
réel est `app_xxx.m.ng_yyy.cc-ng.cloud`. Deux étages :

1. **Réécriture statique** des variables d'environnement au moment du plan
   (URLs et motifs `host:port`).
2. **Stub DNS dans le shim** pour les noms codés en dur dans les images (configs
   nginx, fichiers applicatifs).

Le nom étant connu au plan et stable tant que le membre reste dans le groupe, le
shim n'appelle jamais l'API : **pas de token dans le conteneur, pas de
dépendance de démarrage, pas de retry.**

**La table DNS va dans les variables d'environnement, pas dans l'image.** La
mettre dans l'image ferait changer son hash à chaque modification de topologie,
donc rebuild et push. Format :
`CC_COMPOSE_DNS_MAP=db=app_xxx.m.ng_yyy.cc-ng.cloud,cache=app_zzz...`.
Un changement de topologie ne provoque alors qu'un redémarrage.

Ne jamais utiliser `/etc/hosts` : les IPs des peers changent au blue/green et au
scaling. Un CNAME respecte le TTL, un fichier hosts se périme.

### Ordonnancement du `up`

Créer les apps sans déployer, créer le groupe, lier, lire les FQDN, poser les
variables, déployer dans l'ordre topologique. Le déploiement est en dernier
parce que la table doit être complète avant le premier démarrage.

Cas incrémental : l'ajout d'un service laisse les apps existantes avec une table
incomplète. Comparer la table calculée à celle posée sur chaque app et ne
redémarrer que celles dont l'environnement change réellement.

### Diagnostic obligatoire sur les proxys

Un nginx avec `proxy_pass http://app:3000` résout une fois au démarrage puis
renvoie des 502 au premier redéploiement de `app`. Détecter les images nginx,
haproxy et traefik et émettre un diagnostic imposant une directive `resolver`
plus une variable dans la configuration.

### État

Pas de fichier d'état local. Nommer les apps `<projet>-<service>` et poser des
tags (`compose-project`, `compose-service`, hash de la spécification traduite).
L'état se reconstruit depuis l'API, ce qui rend la réconciliation sans état et
utilisable depuis n'importe quel poste ou une CI.

Dépôts synthétiques construits en mémoire avec `gix`, commits déterministes
(auteur et date fixes, contenu trié). Deux `up` sans changement produisent le
même hash d'arbre, donc le push se saute.

---

## 8. Le shim

Binaire statique musl, PID 1, dans le conteneur de l'utilisateur.

```dockerfile
FROM postgres:16
COPY cc-shim /cc-shim
COPY cc-shim.json /etc/cc-shim.json
USER root
ENTRYPOINT ["/cc-shim"]
```

Séquence : lire la config figée, démarrer le stub DNS et réécrire `resolv.conf`,
matérialiser les secrets dans `/run/secrets`, ouvrir le relais TCP si besoin,
rendre les privilèges, exécuter l'enfant, relayer les signaux et récupérer les
zombies.

**Relais de port.** Seule partie non négociable, puisque
`CC_DOCKER_EXPOSED_HTTP_PORT` est interdit. Écoute sur 8080, relaie vers le port
réel. Une centaine de lignes de tokio.

**Config figée au build.** Redéfinir `ENTRYPOINT` dans une image dérivée remet à
vide le `CMD` hérité. Lire la configuration de l'image sur le registre au moment
du plan (crate `oci-client`, sans démon Docker) et figer la commande complète
dans `cc-shim.json`.

**Passage de privilèges.** En musl statique, `getpwnam` n'est pas fiable, NSS
reposant sur le chargement dynamique. Résoudre `postgres` en uid/gid **au
build**, dans une étape du Dockerfile généré qui écrit le résultat dans un
fichier. Ne pas oublier `initgroups` : basculer uid et gid principal seulement
casse les images qui dépendent d'un groupe secondaire.

**Stub DNS.** `hickory-dns` pour le parsing et le forwarding, la table et la
réécriture restant à écrire. Règles :

- ne rien cacher, résoudre en amont à chaque requête et propager le TTL
  d'origine (le résolveur de la VM cache déjà, un cache local survivant à un
  blue/green renverrait des IPs mortes) ;
- lire les nameservers du `resolv.conf` d'origine avant réécriture, s'en servir
  comme amont, écrire un fichier **sans `search`** pour que `db` arrive sous la
  forme exacte `db.` (matching par comparaison exacte) ;
- écouter sur une adresse de loopback dédiée (127.0.0.153, en évitant
  127.0.0.53), traiter `EADDRINUSE` comme une erreur explicite ;
- pour un nom de la table, renvoyer le CNAME plus les A résolus en amont ; sur
  une requête AAAA, laisser passer la réponse amont y compris un NODATA, un
  NXDOMAIN cassant les clients qui tentent AAAA puis A.

**PID 1.** Relayer SIGTERM et SIGINT, récupérer les zombies, propager le code de
sortie. Sémantique de référence : tini.

**Version minimale d'abord.** Relais de port, signaux, exec : environ trois
cents lignes. N'ajouter le stub DNS que quand un compose réel du corpus échoue à
cause d'un nom codé en dur (probable avec nginx ou Sentry, mais que ce soit un
test qui le dise).

---

## 9. Parsing

Pas de dépendance runtime à la CLI Docker. Le schéma typé vient de
`compose_spec`, qui valide réellement les valeurs et convertit entre syntaxes
courte et longue. Ce qui reste à écrire est la **normalisation** :

1. **Découverte et fusion des fichiers.** `compose.yaml` puis
   `compose.override.yaml`, plus les `-f`. La fusion n'est pas un merge JSON :
   les séquences se remplacent pour `command` et `entrypoint`, se concatènent
   pour `ports` et `volumes`, les mappings fusionnent clé à clé. Une erreur de
   règle donne un déploiement faux sans aucun message.
2. **`include` puis `extends`.** Récursifs, inter-fichiers, mêmes règles de
   fusion, détection de cycles.
3. **Interpolation.** Sur les scalaires de l'arbre déjà parsé, **jamais sur le
   texte brut**. Quatre formes (`:-`, `-`, `:?`, `?`), échappement `$$`,
   précédence environnement du shell puis `.env` du projet. `env_file` alimente
   le conteneur, pas l'interpolation.
4. **Profils.** Service sans `profiles` toujours actif ; `depends_on` vers un
   service inactif est une erreur.
5. **Canonicalisation** vers les formes longues.

**Piège YAML.** La référence Go (`yaml.v3`) est à mi-chemin entre YAML 1.1 et
1.2. Les parseurs Rust résolvent en 1.2. Points de divergence à traiter comme
une spécification à part, pas comme un détail de bibliothèque : booléens
(`yes`/`no`/`on`/`off`) et surtout sexagésimal (`22:22` lu comme un entier en
1.1). `serde_yaml` étant archivé, vérifier le comportement de résolution du
successeur retenu (`serde_yaml_ng`, `serde_norway`, ou `saphyr` directement)
avant de s'engager. Un parseur donnant accès aux nœuds bruts simplifie
l'interpolation.

---

## 10. Exports

**Terraform.** Chaque ressource Docker reçoit un bloc `networkgroups` avec
`networkgroup_id` et `fqdn`. Le nom de la ressource qui crée le groupe est à
récupérer depuis `terraform providers schema -json`, pas depuis la mémoire.
Générer et valider le HCL contre ce schéma en CI, avec `terraform validate` sur
des fixtures, pour ne pas courir après les évolutions du provider.

**Aucun secret en littéral.** Si l'expansion décrite en 6.2 est retenue, les
identifiants d'addon n'apparaissent jamais dans le HCL : l'environnement généré
porte des références non résolues, et le lien s'occupe du reste. C'est une
décision qui contraint le format de sortie, donc à trancher avant d'écrire
l'émetteur, pas après.

Les dépôts synthétiques doivent être hébergés pour que le bloc `deployment`
puisse les référencer : les pousser comme branches dédiées
(`clever-compose/<service>`) du dépôt de l'utilisateur.

Option à considérer : construire d'abord la représentation JSON (Terraform
accepte nativement `.tf.json`), valider dessus, et générer le HCL par templating
depuis la même structure. Évite les règles d'échappement tout en gardant un
export lisible.

**Script clever-tools.** Fondamentalement non idempotent en shell. Le présenter
comme un bootstrap à usage unique, Terraform étant l'export réconciliable.
Séquence de référence :

```bash
clever features enable ng
clever ng create <label> --link <app_id_1>,<app_id_2>
clever ng get <app_id> --format json | jq -r '.domainName'
```

---

## 11. Stratégie de test

**Test différentiel sur le parsing.** `docker compose config` sert d'oracle en
CI, jamais de dépendance runtime. Un corpus produit le JSON normalisé avec la
CLI de référence et avec notre implémentation, comparaison après
canonicalisation. Les divergences deviennent des échecs de test au lieu de
tickets clients.

**Corpus** (plus important que le code) : WordPress, Sentry self-hosted (ancres
YAML et `extends`), Mastodon, Plausible, Immich, Nextcloud, Gitea. Plus des cas
synthétiques pour chaque règle de fusion et chaque forme d'interpolation, et des
cas dédiés aux divergences de résolution YAML.

**Golden files sur le plan.** Chaque compose du corpus produit un plan attendu.
Les émetteurs se testent sur ces plans, hors ligne.

**Équivalence des backends.** Le même plan appliqué par l'exécuteur et par
`terraform apply` doit donner le même état.

---

## 12. Phases

### Phase 0 : validation des hypothèses conteneur

Contre-intuitif dans une méthodologie phasée, mais c'est là que sont toutes les
inconnues qui peuvent invalider l'architecture. Un shim minimal (relais de port,
signaux, exec) déployé à la main sur une application Docker avec `postgres:16`.

*Sortie :* réponse écrite aux points 3, 4, 5 et 6 de la section 4. Un
`postgres:16` joignable depuis une autre application du même network group.

### Phase 1 : parsing, plan et diagnostics

Dry-run pur, aucune API. Le plan est la seule sortie.

*Sortie :* le corpus passe le test différentiel ; chaque clé refusée produit un
diagnostic explicite avec le chemin dans le fichier source.

### Phase 2 : les deux émetteurs

Validables hors ligne sur les golden files.

*Sortie :* `terraform validate` passe sur les fixtures ; le script généré est
relu manuellement sur trois cas.

### Phase 3 : exécuteur et tags

Création, réconciliation depuis l'API, dépôts synthétiques déterministes,
déploiement dans l'ordre topologique.

*Sortie :* un compose à deux services sans dépendance réseau déployé et
redéployé sans changement, avec push sauté à la seconde exécution.

### Phase 4 : network groups et DNS

Création du groupe, liaison, lecture des FQDN, réécriture statique, puis stub
DNS seulement si la phase 0 a montré qu'il est nécessaire.

*Sortie :* un compose WordPress complet fonctionne sans modification du fichier
source.

### Phase 5 : substitution par les addons

La plus heuristique, donc la dernière, et la seule à se subdiviser. Les deux
premières sous-phases suffisent à faire tourner WordPress ; la troisième est ce
qui fait passer Sentry ou Mastodon.

**5a. Reconnaissance et versions.** Table image vers fournisseur, dialectes de
variables par variante, mapping des tags vers les versions disponibles, coût
mensuel affiché dans `plan`.
*Sortie :* aucune substitution silencieuse ; toute version indisponible et toute
image construite produisent un diagnostic.

**5b. Câblage des identifiants.** Réécriture des URLs, corrélation par valeur
sur les variables discrètes, formats non standards, et expansion par le shim si
la question 7 de la section 4 le rend nécessaire.
*Sortie :* les consommateurs du corpus se connectent sans intervention
manuelle ; aucun secret littéral dans l'environnement généré ni dans l'export.

**5c. Initialisation et reprise de données.** Tâche d'exécution des scripts
`docker-entrypoint-initdb.d` avec table témoin, commande `seed`.
*Sortie :* un compose avec scripts d'init démarre sur une base peuplée ; deux
`up` successifs ne rejouent pas les scripts.

**5d. Diagnostics de falaise.** Superutilisateur, bases multiples, arguments de
tuning, limites de connexions, latence du proxy.
*Sortie :* chaque falaise de la section 6.4 a un diagnostic nommé et un test.

---

## 13. Règles anti-dérive

- Le plan intermédiaire est le contrat. Toute évolution qui le touche se discute
  avant d'être codée.
- Aucune clé Compose n'est approximée. Soit elle est traduite fidèlement, soit
  elle produit un diagnostic. L'entre-deux est interdit.
- Les contraintes de la section 3 sont issues de la documentation. Ne pas les
  contredire sur la base d'un souvenir : vérifier la source ou tester.
- Ce qui est fetché ou dérivé (schéma du provider, config d'une image) ne se
  code pas en dur, se relit.
- Chaque phase met à jour `summary.md` avec ce qui a été décidé, ce qui a été
  invalidé, et les questions ouvertes restantes.
