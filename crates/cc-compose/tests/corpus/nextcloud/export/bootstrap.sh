#!/usr/bin/env bash
# Bootstrap généré par clever-compose pour le projet `nextcloud` (spec a8756149457c4f7cfba9e5a2388e71c4).
# À usage unique : ce script crée les ressources, il ne les réconcilie pas.
# Les dépôts synthétiques sont attendus dans ./repos/<service> (export).
set -euo pipefail
ORG="${CC_ORG:?organisation Clever Cloud requise (orga_xxx ou user_xxx)}"
cd "$(dirname "$0")"

# Network groups (un par réseau Compose)
clever features enable ng >/dev/null 2>&1 || true
clever ng create 'nextcloud-default' -o "$ORG" --tags 'compose-project=nextcloud,compose-network=default,compose-spec=a8756149457c4f7cfba9e5a2388e71c4'
NG_default="$(clever ng get 'nextcloud-default' -o "$ORG" -F json | jq -r .id)"

# Applications, créées sans déploiement
APP_nc="$(clever create -t docker 'nextcloud-nc' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_nc" --flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=nextcloud
# tag : compose-service=nc
# tag : compose-spec=a8756149457c4f7cfba9e5a2388e71c4
APP_db="$(clever create -t docker 'nextcloud-db' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_db" --flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=nextcloud
# tag : compose-service=db
# tag : compose-spec=a8756149457c4f7cfba9e5a2388e71c4

# Liaisons aux network groups
clever ng link "$APP_nc" 'nextcloud-default' -o "$ORG"
clever ng link "$APP_db" 'nextcloud-default' -o "$ORG"

# Variables d'environnement (les FQDN des membres sont connus dès la création)
clever env set --app "$APP_nc" 'CC_COMPOSE_DNS_MAP' 'db='"${APP_db}.m.${NG_default}.cc-ng.cloud"
clever env set --app "$APP_nc" 'POSTGRES_DB' 'nextcloud'
clever env set --app "$APP_nc" 'POSTGRES_HOST' "${APP_db}.m.${NG_default}.cc-ng.cloud"
clever env set --app "$APP_nc" 'POSTGRES_PASSWORD' 'nextcloud'
clever env set --app "$APP_nc" 'POSTGRES_USER' 'nextcloud'
clever env set --app "$APP_db" 'POSTGRES_DB' 'nextcloud'
clever env set --app "$APP_db" 'POSTGRES_PASSWORD' 'nextcloud'
clever env set --app "$APP_db" 'POSTGRES_USER' 'nextcloud'

# Déploiements, dans l'ordre topologique (dépôts synthétiques exportés)
(
  cd repos/'nc'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_nc"
)
(
  cd repos/'db'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_db"
)
