# summary.md : journal des décisions

Mis à jour à la fin de chaque phase (règle anti-dérive, CLAUDE.md §13).
Trois rubriques par phase : décidé, invalidé, questions ouvertes.

---

## État courant

Phase 0 en cours. Aucun code. Le dépôt contient le cadrage (`CLAUDE.md`) et ce
journal.

## Accès plateforme

- clever-tools 5.0.0 installé, profil connecté (OAuth 1.0 dans
  `~/.config/clever-cloud/clever-tools.json`). Suffisant pour la phase 0.
- Le SDK Rust lira le même fichier en phase 3. Pas de token supplémentaire prévu.

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
