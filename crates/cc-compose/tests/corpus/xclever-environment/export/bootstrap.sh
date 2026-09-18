#!/usr/bin/env bash
# Bootstrap généré par clever-compose pour le projet `xclever-environment` (spec fbcfa2da4c410c866c34c5f0131504d5).
# À usage unique : ce script crée les ressources, il ne les réconcilie pas.
# Les dépôts synthétiques sont attendus dans ./repos/<service> (export).
set -euo pipefail
ORG="${CLEVER_COMPOSE_ORG:?organisation Clever Cloud requise (orga_xxx ou user_xxx)}"
cd "$(dirname "$0")"

# Network groups (un par réseau Compose)
clever features enable ng >/dev/null 2>&1 || true
clever ng create 'xclever-environment-default' -o "$ORG" --tags 'compose-project=xclever-environment,compose-network=default,compose-spec=fbcfa2da4c410c866c34c5f0131504d5'
NG_default="$(clever ng get 'xclever-environment-default' -o "$ORG" -F json | jq -r .id)"

# Addons managés (facturés selon le plan)
clever addon create 'postgresql-addon' 'xclever-environment-db1' --plan 'xxs_sml' --region 'par' --addon-version '18' -o "$ORG"
ADDON_db1="$(clever addon list -o "$ORG" -F json | jq -r '.[] | select(.name == "xclever-environment-db1") | .addonId')"
ADDON_REAL_db1="$(clever addon list -o "$ORG" -F json | jq -r '.[] | select(.name == "xclever-environment-db1") | .realId')"
clever addon create 'postgresql-addon' 'xclever-environment-db2' --plan 'xxs_sml' --region 'par' --addon-version '18' -o "$ORG"
ADDON_db2="$(clever addon list -o "$ORG" -F json | jq -r '.[] | select(.name == "xclever-environment-db2") | .addonId')"
ADDON_REAL_db2="$(clever addon list -o "$ORG" -F json | jq -r '.[] | select(.name == "xclever-environment-db2") | .realId')"

# Applications, créées sans déploiement
APP_app="$(clever create -t docker 'xclever-environment-app' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_app" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=xclever-environment
# tag : compose-service=app
# tag : compose-spec=fbcfa2da4c410c866c34c5f0131504d5

# Liaisons aux network groups
clever ng link "$APP_app" 'xclever-environment-default' -o "$ORG"
clever ng link "$ADDON_REAL_db1" 'xclever-environment-default' -o "$ORG"
clever ng link "$ADDON_REAL_db2" 'xclever-environment-default' -o "$ORG"

# Liaisons aux addons (injection de leurs variables)
clever service link-addon "$ADDON_db1" --app "$APP_app"
clever service link-addon "$ADDON_db2" --app "$APP_app"

# Variables d'environnement (les FQDN des membres sont connus dès la création)
clever env set --app "$APP_app" 'APP_DB_HOST' "${ADDON_REAL_db2}.m.${NG_default}.cc-ng.cloud"':8080'
clever env set --app "$APP_app" 'APP_DB_PASSWORD' '${POSTGRESQL_ADDON_PASSWORD}'
clever env set --app "$APP_app" 'APP_DB_URL' 'postgres://''${POSTGRESQL_ADDON_USER}'':''${POSTGRESQL_ADDON_PASSWORD}''@'"${ADDON_REAL_db2}.m.${NG_default}.cc-ng.cloud"':''8080''/''${POSTGRESQL_ADDON_DB}''?sslmode=disable'
clever env set --app "$APP_app" 'APP_DB_USER' '${POSTGRESQL_ADDON_USER}'
clever env set --app "$APP_app" 'APP_SECRET' 'literal-sur-clever'
clever env set --app "$APP_app" 'CLEVER_COMPOSE_DNS_MAP' 'db1='"${ADDON_REAL_db1}.m.${NG_default}.cc-ng.cloud"',db2='"${ADDON_REAL_db2}.m.${NG_default}.cc-ng.cloud"
clever env set --app "$APP_app" 'CLEVER_COMPOSE_EXPAND' 'APP_DB_PASSWORD,APP_DB_USER,APP_DB_URL'

# Déploiements, dans l'ordre topologique (dépôts synthétiques exportés)
(
  cd repos/'app'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_app"
)
