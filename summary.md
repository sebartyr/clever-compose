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
jusqu'au conteneur). **Phase 3 (exécuteur) à ouvrir.**
Dépôt déplacé le 13/09/2026 dans `~/cc-compose/clever-compose`, avec les
sources de Compose dans `~/cc-compose/compose`, Axo dans `~/axo`, OVD dans
`~/ovd`. Remote `origin` : `github.com/sebartyr/clever-compose`.

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

- Rendu HCL lisible par templating depuis le JSON (confort, pas bloquant).
- `deployment.repository` : Terraform attend que les branches
  `clever-compose/<service>` soient poussées sur un dépôt de l'utilisateur
  (§10) ; c'est l'exécuteur ou `export` qui les poussera (phase 3).

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
