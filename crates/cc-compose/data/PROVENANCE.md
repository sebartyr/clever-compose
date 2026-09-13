# docker-instance.json

Extrait de `GET https://api.clever-cloud.com/v2/products/instances`, entrée
`type == "docker"` (version `20260818`), lu le 13/09/2026 : flavors avec
mémoire en octets et CPU, flavor par défaut, nombre maximal d'instances.

Sert au choix de la flavor la plus proche en mémoire (CLAUDE.md §6) en
dry-run. L'exécuteur (phase 3) relira l'API et signalera un écart avec ce
fichier. Régénérer avec la même requête ; ne pas éditer à la main.

# terraform-provider-schema.json

Sortie de `terraform providers schema -json` (Terraform 1.16.2) pour le
provider `CleverCloud/clevercloud` **2.1.0**, le 13/09/2026, réduite aux
ressources utilisées (`clevercloud_docker`, `clevercloud_networkgroup`, les
addons `postgresql`, `mysql`, `redis`, `mongodb`, et `configprovider`).

Sert à l'émetteur Terraform pour les noms de ressources et d'attributs
(CLAUDE.md §10 : jamais de mémoire). Régénérer avec un `main.tf` qui
déclare le provider, `terraform init`, puis la commande ci-dessus.

# addon-providers.json

Extrait de `GET /v2/products/addonproviders` (plans, prix mensuels en euros
HT, zones, features dont `Max connection limit` et `Memory`) et de
`GET /v4/addon-providers/{id}` (versions dédiées et version par défaut), le
13/09/2026, pour `postgresql-addon`, `mysql-addon`, `redis-addon`,
`mongodb-addon`, `es-addon`, `cellar-addon`, `config-provider`.

Sert au planificateur en dry-run (reconnaissance, choix de plan, coût
annoncé, versions). L'exécuteur relit l'API. Régénérer par le script de
`summary.md` (phase 5) ; ne pas éditer à la main.
