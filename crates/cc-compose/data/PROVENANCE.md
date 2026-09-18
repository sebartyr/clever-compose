# docker-instance.json

Extrait de `GET https://api.clever-cloud.com/v2/products/instances`, entrée
`type == "docker"` (version `20260818`), lu le 13/09/2026 : flavors avec
mémoire en octets et CPU, flavor par défaut, nombre maximal d'instances.

Sert au choix de la flavor la plus proche en mémoire (CLAUDE.md §6) en
dry-run. L'exécuteur (phase 3) relira l'API et signalera un écart avec ce
fichier. Régénérer avec la même requête ; ne pas éditer à la main.

# terraform-provider-schema.json

Sortie de `terraform providers schema -json` (Terraform 1.16.2) pour le
provider `CleverCloud/clevercloud` **2.2.0**, le 16/09/2026, réduite aux
ressources utilisées (`clevercloud_docker`, `clevercloud_networkgroup`, les
addons `postgresql`, `mysql`, `redis`, `mongodb`, `elasticsearch`,
`configprovider`, et depuis le 18/09/2026 `cellar` et `cellar_bucket`,
relus dans le schéma régénéré du même provider 2.2.0). Les sources du provider étaient en local dans
`~/cc-compose/terraform-provider-clevercloud` (absentes du poste le
18/09/2026 : les recloner pour lire le comportement derrière le schéma,
`pkg/resources/application/git.go` pour `deployment`,
`pkg/resources/ng.go` pour `networkgroups`, `dependencies.go`).

Sert à l'émetteur Terraform pour les noms de ressources et d'attributs
(CLAUDE.md §10 : jamais de mémoire). Régénérer avec un `main.tf` qui
déclare le provider, `terraform init`, puis la commande ci-dessus.

# addon-providers.json

Extrait de `GET /v2/products/addonproviders` (plans, prix mensuels en euros
HT, zones, features dont `Max connection limit` et `Memory`) et de
`GET /v4/addon-providers/{id}` (versions dédiées et version par défaut, et
`shared_versions` : version du cluster partagé par zone, lue dans `clusters`,
ajoutée le 16/09/2026 après un `dev` demandé en 16 et servi en 15), le
13/09/2026, pour `postgresql-addon`, `mysql-addon`, `redis-addon`,
`mongodb-addon`, `es-addon`, `cellar-addon`, `config-provider`.

Sert au planificateur en dry-run (reconnaissance, choix de plan, coût
annoncé, versions). L'exécuteur relit l'API. Régénérer par le script de
`summary.md` (phase 5) ; ne pas éditer à la main.

# addon-images.json

Règles de reconnaissance des images substituables et de lecture de leurs
identifiants, dérivées des entrypoints des images (18/09/2026) :
`docker-library/postgres` (`docker_setup_env` : `POSTGRES_USER` défaut
`postgres`, `POSTGRES_DB` défaut `$POSTGRES_USER`), `docker-library/mysql`
et `mariadb` (`MYSQL_*`, `MARIADB_*` acceptés par les deux), `redis`
(`--requirepass` dans la commande), `docker-library/mongo`
(`MONGO_INITDB_*`), `elastic` (utilisateur fixe `elastic`,
`ELASTIC_PASSWORD`), et les variantes Bitnami (`POSTGRESQL_*`,
`MONGODB_*`, `REDIS_PASSWORD`).

`dialects` : par nom, fournisseur cible, nom court, port du conteneur,
règle de version depuis le tag (`major` ou `major.minor`), et pour chaque
rôle (`user`, `password`, `database`, `root_password`) la source :
`env` (liste de variables, la première renseignée), `default` (littéral)
ou `default_from` (un autre rôle), `command_flag` (valeur qui suit le
drapeau dans `command`), `fixed`. `images` : nom d'image (sans registre
`docker.io/library/`) vers dialecte. Un service peut désigner un dialecte
par `x-clever.dialect` (image custom). Relire les entrypoints avant
d'ajouter une image ; jamais de souvenir.
