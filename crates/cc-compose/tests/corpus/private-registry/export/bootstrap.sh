#!/usr/bin/env bash
# Bootstrap généré par clever-compose pour le projet `private-registry` (spec 2e1635890a217ca2cfc876398bf542ca).
# À usage unique : ce script crée les ressources, il ne les réconcilie pas.
# Les dépôts synthétiques sont attendus dans ./repos/<service> (export).
set -euo pipefail
ORG="${CC_ORG:?organisation Clever Cloud requise (orga_xxx ou user_xxx)}"
cd "$(dirname "$0")"

# Network groups (un par réseau Compose)
clever features enable ng >/dev/null 2>&1 || true
clever ng create 'private-registry-default' -o "$ORG" --tags 'compose-project=private-registry,compose-network=default,compose-spec=2e1635890a217ca2cfc876398bf542ca'
NG_default="$(clever ng get 'private-registry-default' -o "$ORG" -F json | jq -r .id)"

# Addons managés (facturés selon le plan)
clever addon create 'es-addon' 'private-registry-search' --plan 'xs' --region 'par' --addon-version '8' -o "$ORG"
ADDON_search="$(clever addon list -o "$ORG" -F json | jq -r '.[] | select(.name == "private-registry-search") | .addonId')"

# Applications, créées sans déploiement
APP_app="$(clever create -t docker 'private-registry-app' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_app" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=private-registry
# tag : compose-service=app
# tag : compose-spec=2e1635890a217ca2cfc876398bf542ca

# Liaisons aux network groups
clever ng link "$APP_app" 'private-registry-default' -o "$ORG"

# Liaisons aux addons (injection de leurs variables)
clever service link-addon "$ADDON_search" --app "$APP_app"

# Variables d'environnement (les FQDN des membres sont connus dès la création)
clever env set --app "$APP_app" 'CC_COMPOSE_EXPAND' 'ES_URL'
clever env set --app "$APP_app" 'ES_URL' 'https://''${ES_ADDON_USER}'':''${ES_ADDON_PASSWORD}''@''${ES_ADDON_HOST}'

# Déploiements, dans l'ordre topologique (dépôts synthétiques exportés)
(
  cd repos/'app'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_app"
)
