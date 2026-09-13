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

Prochaine étape : phase 1 (parsing, plan, diagnostics).

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
  `<ngId><suffix>`, cas dans `tests/dns_parity.rs`). Pas encore posté.

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
