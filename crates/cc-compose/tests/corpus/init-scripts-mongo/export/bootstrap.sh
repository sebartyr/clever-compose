#!/usr/bin/env bash
# Bootstrap généré par clever-compose pour le projet `init-scripts-mongo` (spec 3909e8e24f11e2d03b03f848ab3f283c).
# À usage unique : ce script crée les ressources, il ne les réconcilie pas.
# Les dépôts synthétiques sont attendus dans ./repos/<service> (export).
set -euo pipefail
ORG="${CC_ORG:?organisation Clever Cloud requise (orga_xxx ou user_xxx)}"
cd "$(dirname "$0")"

# Network groups (un par réseau Compose)
clever features enable ng >/dev/null 2>&1 || true
clever ng create 'init-scripts-mongo-default' -o "$ORG" --tags 'compose-project=init-scripts-mongo,compose-network=default,compose-spec=3909e8e24f11e2d03b03f848ab3f283c'
NG_default="$(clever ng get 'init-scripts-mongo-default' -o "$ORG" -F json | jq -r .id)"

# Addons managés (facturés selon le plan)
clever addon create 'mongodb-addon' 'init-scripts-mongo-mongo' --plan 'xs_sml' --region 'par' --version '4.0.3' -o "$ORG"
ADDON_mongo="$(clever addon list -o "$ORG" -F json | jq -r '.[] | select(.name == "init-scripts-mongo-mongo") | .addonId')"
clever addon create 'redis-addon' 'init-scripts-mongo-cache' --plan 's_mono' --region 'par' --version '8.10.1' -o "$ORG"
ADDON_cache="$(clever addon list -o "$ORG" -F json | jq -r '.[] | select(.name == "init-scripts-mongo-cache") | .addonId')"

# Applications, créées sans déploiement
APP_mongo_init="$(clever create -t docker 'init-scripts-mongo-mongo-init' -o "$ORG" -r 'par' --task '' -F json | jq -r .id)"
clever scale --app "$APP_mongo_init" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=init-scripts-mongo
# tag : compose-service=mongo-init
# tag : compose-spec=3909e8e24f11e2d03b03f848ab3f283c
APP_api="$(clever create -t docker 'init-scripts-mongo-api' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_api" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=init-scripts-mongo
# tag : compose-service=api
# tag : compose-spec=3909e8e24f11e2d03b03f848ab3f283c

# Liaisons aux network groups
clever ng link "$APP_api" 'init-scripts-mongo-default' -o "$ORG"

# Liaisons aux addons (injection de leurs variables)
clever service link-addon "$ADDON_mongo" --app "$APP_mongo_init"
clever service link-addon "$ADDON_mongo" --app "$APP_api"

# Variables d'environnement (les FQDN des membres sont connus dès la création)
clever env set --app "$APP_mongo_init" 'CC_COMPOSE_INIT_HASH' '965bdbffd89e76fcb0544c283a25b57e'
clever env set --app "$APP_api" 'CC_COMPOSE_EXPAND' 'MONGO_URL'
clever env set --app "$APP_api" 'MONGO_URL' 'mongodb://''${MONGODB_ADDON_USER}'':''${MONGODB_ADDON_PASSWORD}''@''${MONGODB_ADDON_HOST}'':''${MONGODB_ADDON_PORT}''/''${MONGODB_ADDON_DB}'

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
