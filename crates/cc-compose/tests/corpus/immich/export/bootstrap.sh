#!/usr/bin/env bash
# Bootstrap généré par clever-compose pour le projet `immich` (spec 9c4e555a4763a27270cdeeefe61bd634).
# À usage unique : ce script crée les ressources, il ne les réconcilie pas.
# Les dépôts synthétiques sont attendus dans ./repos/<service> (export).
set -euo pipefail
ORG="${CLEVER_COMPOSE_ORG:?organisation Clever Cloud requise (orga_xxx ou user_xxx)}"
cd "$(dirname "$0")"

# Network groups (un par réseau Compose)
clever features enable ng >/dev/null 2>&1 || true
clever ng create 'immich-default' -o "$ORG" --tags 'compose-project=immich,compose-network=default,compose-spec=9c4e555a4763a27270cdeeefe61bd634'
NG_default="$(clever ng get 'immich-default' -o "$ORG" -F json | jq -r .id)"

# Applications, créées sans déploiement
APP_redis="$(clever create -t docker 'immich-redis' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_redis" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=immich
# tag : compose-service=redis
# tag : compose-spec=9c4e555a4763a27270cdeeefe61bd634
APP_database="$(clever create -t docker 'immich-database' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_database" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=immich
# tag : compose-service=database
# tag : compose-spec=9c4e555a4763a27270cdeeefe61bd634
APP_immich_server="$(clever create -t docker 'immich-immich-server' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_immich_server" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=immich
# tag : compose-service=immich-server
# tag : compose-spec=9c4e555a4763a27270cdeeefe61bd634
APP_immich_machine_learning="$(clever create -t docker 'immich-immich-machine-learning' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_immich_machine_learning" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=immich
# tag : compose-service=immich-machine-learning
# tag : compose-spec=9c4e555a4763a27270cdeeefe61bd634

# Liaisons aux network groups
clever ng link "$APP_immich_server" 'immich-default' -o "$ORG"
clever ng link "$APP_immich_machine_learning" 'immich-default' -o "$ORG"
clever ng link "$APP_redis" 'immich-default' -o "$ORG"
clever ng link "$APP_database" 'immich-default' -o "$ORG"

# Variables d'environnement (les FQDN des membres sont connus dès la création)
clever env set --app "$APP_database" 'POSTGRES_DB' 'immich'
clever env set --app "$APP_database" 'POSTGRES_INITDB_ARGS' '--data-checksums'
clever env set --app "$APP_database" 'POSTGRES_PASSWORD' 'postgres'
clever env set --app "$APP_database" 'POSTGRES_USER' 'postgres'
clever env set --app "$APP_immich_server" 'CLEVER_COMPOSE_DNS_MAP' 'database='"${APP_database}.m.${NG_default}.cc-ng.cloud"',redis='"${APP_redis}.m.${NG_default}.cc-ng.cloud"
clever env set --app "$APP_immich_server" 'DB_DATABASE_NAME' 'immich'
clever env set --app "$APP_immich_server" 'DB_DATA_LOCATION' './postgres'
clever env set --app "$APP_immich_server" 'DB_PASSWORD' 'postgres'
clever env set --app "$APP_immich_server" 'DB_USERNAME' 'postgres'
clever env set --app "$APP_immich_server" 'IMMICH_VERSION' 'v3'
clever env set --app "$APP_immich_server" 'UPLOAD_LOCATION' './library'
clever env set --app "$APP_immich_machine_learning" 'DB_DATABASE_NAME' 'immich'
clever env set --app "$APP_immich_machine_learning" 'DB_DATA_LOCATION' './postgres'
clever env set --app "$APP_immich_machine_learning" 'DB_PASSWORD' 'postgres'
clever env set --app "$APP_immich_machine_learning" 'DB_USERNAME' 'postgres'
clever env set --app "$APP_immich_machine_learning" 'IMMICH_VERSION' 'v3'
clever env set --app "$APP_immich_machine_learning" 'UPLOAD_LOCATION' './library'

# Déploiements, dans l'ordre topologique (dépôts synthétiques exportés)
(
  cd repos/'redis'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_redis"
)
(
  cd repos/'database'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_database"
)
(
  cd repos/'immich-server'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_immich_server"
)
(
  cd repos/'immich-machine-learning'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_immich_machine_learning"
)
