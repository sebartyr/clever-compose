# docker-instance.json

Extrait de `GET https://api.clever-cloud.com/v2/products/instances`, entrée
`type == "docker"` (version `20260818`), lu le 13/09/2026 : flavors avec
mémoire en octets et CPU, flavor par défaut, nombre maximal d'instances.

Sert au choix de la flavor la plus proche en mémoire (CLAUDE.md §6) en
dry-run. L'exécuteur (phase 3) relira l'API et signalera un écart avec ce
fichier. Régénérer avec la même requête ; ne pas éditer à la main.
