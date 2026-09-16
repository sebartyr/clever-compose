#!/usr/bin/env bash
# Bootstrap généré par clever-compose pour le projet `gitea` (spec 9aa14d857c9c7c91984add8639f6f607).
# À usage unique : ce script crée les ressources, il ne les réconcilie pas.
# Les dépôts synthétiques sont attendus dans ./repos/<service> (export).
set -euo pipefail
ORG="${CC_ORG:?organisation Clever Cloud requise (orga_xxx ou user_xxx)}"
cd "$(dirname "$0")"

# Network groups (un par réseau Compose)
clever features enable ng >/dev/null 2>&1 || true
clever ng create 'gitea-default' -o "$ORG" --tags 'compose-project=gitea,compose-network=default,compose-spec=9aa14d857c9c7c91984add8639f6f607'
NG_default="$(clever ng get 'gitea-default' -o "$ORG" -F json | jq -r .id)"

# Addons managés (facturés selon le plan)
clever addon create 'postgresql-addon' 'gitea-db' --plan 'xxs_sml' --region 'par' --addon-version '17' -o "$ORG"
ADDON_db="$(clever addon list -o "$ORG" -F json | jq -r '.[] | select(.name == "gitea-db") | .addonId')"
ADDON_REAL_db="$(clever addon list -o "$ORG" -F json | jq -r '.[] | select(.name == "gitea-db") | .realId')"

# Applications, créées sans déploiement
APP_gitea="$(clever create -t docker 'gitea-gitea' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_gitea" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=gitea
# tag : compose-service=gitea
# tag : compose-spec=9aa14d857c9c7c91984add8639f6f607

# Liaisons aux network groups
clever ng link "$APP_gitea" 'gitea-default' -o "$ORG"
clever ng link "$ADDON_REAL_db" 'gitea-default' -o "$ORG"

# Liaisons aux addons (injection de leurs variables)
clever service link-addon "$ADDON_db" --app "$APP_gitea"

# Variables d'environnement (les FQDN des membres sont connus dès la création)
clever env set --app "$APP_gitea" 'CC_COMPOSE_DNS_MAP' 'db='"${ADDON_REAL_db}.m.${NG_default}.cc-ng.cloud"
clever env set --app "$APP_gitea" 'CC_COMPOSE_EXPAND' 'DB_NAME,DB_USER,DB_PASSWD'
clever env set --app "$APP_gitea" 'DB_HOST' "${ADDON_REAL_db}.m.${NG_default}.cc-ng.cloud"':8080'
clever env set --app "$APP_gitea" 'DB_NAME' '${POSTGRESQL_ADDON_DB}'
clever env set --app "$APP_gitea" 'DB_PASSWD' '${POSTGRESQL_ADDON_PASSWORD}'
clever env set --app "$APP_gitea" 'DB_TYPE' 'postgres'
clever env set --app "$APP_gitea" 'DB_USER' '${POSTGRESQL_ADDON_USER}'

# Déploiements, dans l'ordre topologique (dépôts synthétiques exportés)
(
  cd repos/'gitea'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_gitea"
)
