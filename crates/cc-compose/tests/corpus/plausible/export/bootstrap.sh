#!/usr/bin/env bash
# Bootstrap généré par clever-compose pour le projet `plausible` (spec 9f6f941066f236936479d8a2b1a7a52f).
# À usage unique : ce script crée les ressources, il ne les réconcilie pas.
# Les dépôts synthétiques sont attendus dans ./repos/<service> (export).
set -euo pipefail
ORG="${CLEVER_COMPOSE_ORG:?organisation Clever Cloud requise (orga_xxx ou user_xxx)}"
cd "$(dirname "$0")"

# Network groups (un par réseau Compose)
clever features enable ng >/dev/null 2>&1 || true
clever ng create 'plausible-default' -o "$ORG" --tags 'compose-project=plausible,compose-network=default,compose-spec=9f6f941066f236936479d8a2b1a7a52f'
NG_default="$(clever ng get 'plausible-default' -o "$ORG" -F json | jq -r .id)"

# Addons managés (facturés selon le plan)
clever addon create 'postgresql-addon' 'plausible-plausible_db' --plan 'xxs_sml' --region 'par' --addon-version '16' -o "$ORG"
ADDON_plausible_db="$(clever addon list -o "$ORG" -F json | jq -r '.[] | select(.name == "plausible-plausible_db") | .addonId')"
ADDON_REAL_plausible_db="$(clever addon list -o "$ORG" -F json | jq -r '.[] | select(.name == "plausible-plausible_db") | .realId')"

# Applications, créées sans déploiement
APP_plausible_events_db="$(clever create -t docker 'plausible-plausible_events_db' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_plausible_events_db" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=plausible
# tag : compose-service=plausible_events_db
# tag : compose-spec=9f6f941066f236936479d8a2b1a7a52f
APP_plausible="$(clever create -t docker 'plausible-plausible' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_plausible" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=plausible
# tag : compose-service=plausible
# tag : compose-spec=9f6f941066f236936479d8a2b1a7a52f

# Liaisons aux network groups
clever ng link "$ADDON_REAL_plausible_db" 'plausible-default' -o "$ORG"
clever ng link "$APP_plausible_events_db" 'plausible-default' -o "$ORG"
clever ng link "$APP_plausible" 'plausible-default' -o "$ORG"

# Liaisons aux addons (injection de leurs variables)
clever service link-addon "$ADDON_plausible_db" --app "$APP_plausible"

# Variables d'environnement (les FQDN des membres sont connus dès la création)
clever env set --app "$APP_plausible_events_db" 'CC_HEALTH_CHECK_PATH' '/ping'
clever env set --app "$APP_plausible_events_db" 'CLICKHOUSE_SKIP_USER_SETUP' '1'
clever env set --app "$APP_plausible" 'BASE_URL' ''
clever env set --app "$APP_plausible" 'CLEVER_COMPOSE_DNS_MAP' 'plausible_db='"${ADDON_REAL_plausible_db}.m.${NG_default}.cc-ng.cloud"',plausible_events_db='"${APP_plausible_events_db}.m.${NG_default}.cc-ng.cloud"
clever env set --app "$APP_plausible" 'POSTGRESQL_ADDON_NG_HOST' "${ADDON_REAL_plausible_db}.m.${NG_default}.cc-ng.cloud"
clever env set --app "$APP_plausible" 'POSTGRESQL_ADDON_NG_PORT' '8080'
clever env set --app "$APP_plausible" 'SECRET_KEY_BASE' ''
clever env set --app "$APP_plausible" 'TMPDIR' '/var/lib/plausible/tmp'

# Déploiements, dans l'ordre topologique (dépôts synthétiques exportés)
(
  cd repos/'plausible_events_db'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_plausible_events_db"
)
(
  cd repos/'plausible'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_plausible"
)
