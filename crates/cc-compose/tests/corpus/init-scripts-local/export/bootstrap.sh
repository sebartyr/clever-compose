#!/usr/bin/env bash
# Bootstrap généré par clever-compose pour le projet `init-scripts-local` (spec 44c4d252a8e51abc7433e559c5289d27).
# À usage unique : ce script crée les ressources, il ne les réconcilie pas.
# Les dépôts synthétiques sont attendus dans ./repos/<service> (export).
set -euo pipefail
ORG="${CC_ORG:?organisation Clever Cloud requise (orga_xxx ou user_xxx)}"
cd "$(dirname "$0")"

# Network groups (un par réseau Compose)
clever features enable ng >/dev/null 2>&1 || true
clever ng create 'init-scripts-local-default' -o "$ORG" --tags 'compose-project=init-scripts-local,compose-network=default,compose-spec=44c4d252a8e51abc7433e559c5289d27'
NG_default="$(clever ng get 'init-scripts-local-default' -o "$ORG" -F json | jq -r .id)"

# Addons managés (facturés selon le plan)
clever addon create 'postgresql-addon' 'init-scripts-local-db' --plan 'dev' --region 'par' --version '15' -o "$ORG"
ADDON_db="$(clever addon list -o "$ORG" -F json | jq -r '.[] | select(.name == "init-scripts-local-db") | .addonId')"

# Applications, créées sans déploiement
APP_db_init="$(clever create -t docker 'init-scripts-local-db-init' -o "$ORG" -r 'par' --task '' -F json | jq -r .id)"
clever scale --app "$APP_db_init" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=init-scripts-local
# tag : compose-service=db-init
# tag : compose-spec=44c4d252a8e51abc7433e559c5289d27
APP_api="$(clever create -t docker 'init-scripts-local-api' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_api" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=init-scripts-local
# tag : compose-service=api
# tag : compose-spec=44c4d252a8e51abc7433e559c5289d27

# Liaisons aux network groups
clever ng link "$APP_api" 'init-scripts-local-default' -o "$ORG"

# Liaisons aux addons (injection de leurs variables)
clever service link-addon "$ADDON_db" --app "$APP_db_init"
clever service link-addon "$ADDON_db" --app "$APP_api"

# Variables d'environnement (les FQDN des membres sont connus dès la création)
clever env set --app "$APP_db_init" 'CC_COMPOSE_INIT_HASH' '58d214e86fa675d00c5603cc537d6ae5'
clever env set --app "$APP_api" 'CC_COMPOSE_EXPAND' 'DATABASE_URL,DB_PASSWORD'
clever env set --app "$APP_api" 'DATABASE_URL' 'postgres://''${POSTGRESQL_ADDON_USER}'':''${POSTGRESQL_ADDON_PASSWORD}''@''${POSTGRESQL_ADDON_HOST}'':''${POSTGRESQL_ADDON_PORT}''/''${POSTGRESQL_ADDON_DB}''?sslmode=disable'
clever env set --app "$APP_api" 'DB_PASSWORD' '${POSTGRESQL_ADDON_PASSWORD}'

# Déploiements, dans l'ordre topologique (dépôts synthétiques exportés)
(
  cd repos/'db-init'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_db_init"
)
(
  cd repos/'api'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_api"
)
