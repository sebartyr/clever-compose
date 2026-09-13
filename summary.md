# summary.md : journal des décisions

Mis à jour à la fin de chaque phase (règle anti-dérive, CLAUDE.md §13).
Trois rubriques par phase : décidé, invalidé, questions ouvertes.

---

## État courant

Phase 0 en cours. Workspace Rust posé (trois crates), shim minimal écrit
(relais 8080 vers le port réel, PID 1, exec), compilé en statique musl via
`cargo build -p cc-compose-shim --profile shim --target x86_64-unknown-linux-musl`.
Fixture `phase0/postgres` prête, pas encore testée ni déployée.

Choix du shim à valider en phase 0 :
- le relais n'ouvre 8080 qu'une fois le port réel joignable, pour que le
  succès de déploiement vu par la plateforme reflète la disponibilité réelle ;
- abandon de privilèges seulement si l'image déclare `USER`, les identifiants
  numériques étant résolus au build dans `/etc/cc-shim.ids` ;
- `waitpid(-1)` pour récupérer aussi les petits-enfants orphelins.

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
| 1 | Alias de nom (`fqdn` du bloc `networkgroups` Terraform) | ouverte | |
| 2 | FQDN lisible en Terraform pour un apply en une passe | ouverte | |
| 3 | TTL et enregistrements A de `cc-ng.cloud` | ouverte (phase 0) | |
| 4 | Liaison à chaud : redémarrage nécessaire ? | ouverte (phase 0) | |
| 5 | Résolution `cc-ng.cloud` en `--net host`, `resolv.conf` modifiable ? | ouverte (phase 0) | |
| 6 | Workers sans port : critère de succès du déploiement | ouverte (phase 0) | |
| 7 | Expansion des variables entre elles par la plateforme | ouverte (phase 0) | |

Les points 3 à 7 se répondent par l'expérience de phase 0. Les points 1 et 2
par lecture du schéma du provider Terraform et de l'API network groups.

---

## Phase 0 : validation des hypothèses conteneur

*Décidé :* (à compléter)

*Invalidé :* (à compléter)

*Questions ouvertes restantes :* (à compléter)
