#!/usr/bin/env bash
# Bootstrap généré par clever-compose pour le projet `documentdb` (spec c8802af9104689da556fe0634010e647).
# À usage unique : ce script crée les ressources, il ne les réconcilie pas.
# Les dépôts synthétiques sont attendus dans ./repos/<service> (export).
set -euo pipefail
ORG="${CLEVER_COMPOSE_ORG:?organisation Clever Cloud requise (orga_xxx ou user_xxx)}"
cd "$(dirname "$0")"

# Network groups (un par réseau Compose)
clever features enable ng >/dev/null 2>&1 || true
clever ng create 'documentdb-default' -o "$ORG" --tags 'compose-project=documentdb,compose-network=default,compose-spec=c8802af9104689da556fe0634010e647'
NG_default="$(clever ng get 'documentdb-default' -o "$ORG" -F json | jq -r .id)"

# Addons managés (facturés selon le plan)
clever addon create 'postgresql-addon' 'documentdb-mongo' --plan 'xxs_sml' --region 'par' --addon-version '17' --option 'documentdb=true' -o "$ORG"
ADDON_mongo="$(clever addon list -o "$ORG" -F json | jq -r '.[] | select(.name == "documentdb-mongo") | .addonId')"
ADDON_REAL_mongo="$(clever addon list -o "$ORG" -F json | jq -r '.[] | select(.name == "documentdb-mongo") | .realId')"

# Applications, créées sans déploiement
APP_mongo_init="$(clever create -t docker 'documentdb-mongo-init' -o "$ORG" -r 'par' --task '' -F json | jq -r .id)"
clever scale --app "$APP_mongo_init" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=documentdb
# tag : compose-service=mongo-init
# tag : compose-spec=c8802af9104689da556fe0634010e647
APP_api="$(clever create -t docker 'documentdb-api' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_api" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=documentdb
# tag : compose-service=api
# tag : compose-spec=c8802af9104689da556fe0634010e647

# Liaisons aux network groups
clever ng link "$APP_api" 'documentdb-default' -o "$ORG"
clever ng link "$ADDON_REAL_mongo" 'documentdb-default' -o "$ORG"

# Liaisons aux addons (injection de leurs variables)
clever service link-addon "$ADDON_mongo" --app "$APP_mongo_init"
clever service link-addon "$ADDON_mongo" --app "$APP_api"

# Variables d'environnement (les FQDN des membres sont connus dès la création)
clever env set --app "$APP_mongo_init" 'CLEVER_COMPOSE_INIT_HASH' '965bdbffd89e76fcb0544c283a25b57e'
clever env set --app "$APP_api" 'CLEVER_COMPOSE_DNS_MAP' 'mongo='"${ADDON_REAL_mongo}.m.${NG_default}.cc-ng.cloud"
clever env set --app "$APP_api" 'CLEVER_COMPOSE_EXPAND' 'MONGO_URL,MONGO_PASSWORD'
clever env set --app "$APP_api" 'MONGO_HOST' "${ADDON_REAL_mongo}.m.${NG_default}.cc-ng.cloud"
clever env set --app "$APP_api" 'MONGO_PASSWORD' '${POSTGRESQL_ADDON_PASSWORD}'
clever env set --app "$APP_api" 'MONGO_URL' 'mongodb://''${POSTGRESQL_ADDON_USER}'':''${POSTGRESQL_ADDON_PASSWORD}''@'"${ADDON_REAL_mongo}.m.${NG_default}.cc-ng.cloud"':4040''/''${POSTGRESQL_ADDON_DB}''?retryWrites=true&tls=true&tlsAllowInvalidCertificates=true'

# Déploiements, dans l'ordre topologique (dépôts synthétiques exportés)
(
  cd repos/'mongo-init'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_mongo_init"
)
(
  cd repos/'api'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_api"
)
