#!/usr/bin/env bash
# Bootstrap généré par clever-compose pour le projet `wordpress` (spec 5a65e589c9975ae4fcb5adab9d29fab4).
# À usage unique : ce script crée les ressources, il ne les réconcilie pas.
# Les dépôts synthétiques sont attendus dans ./repos/<service> (export).
set -euo pipefail
ORG="${CC_ORG:?organisation Clever Cloud requise (orga_xxx ou user_xxx)}"
cd "$(dirname "$0")"

# Network groups (un par réseau Compose)
clever features enable ng >/dev/null 2>&1 || true
clever ng create 'wordpress-default' -o "$ORG" --tags 'compose-project=wordpress,compose-network=default,compose-spec=5a65e589c9975ae4fcb5adab9d29fab4'
NG_default="$(clever ng get 'wordpress-default' -o "$ORG" -F json | jq -r .id)"

# Applications, créées sans déploiement
APP_db="$(clever create -t docker 'wordpress-db' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_db" --flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=wordpress
# tag : compose-service=db
# tag : compose-spec=5a65e589c9975ae4fcb5adab9d29fab4
APP_wordpress="$(clever create -t docker 'wordpress-wordpress' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_wordpress" --flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=wordpress
# tag : compose-service=wordpress
# tag : compose-spec=5a65e589c9975ae4fcb5adab9d29fab4

# Liaisons aux network groups
clever ng link "$APP_db" 'wordpress-default' -o "$ORG"
clever ng link "$APP_wordpress" 'wordpress-default' -o "$ORG"

# Variables d'environnement (les FQDN des membres sont connus dès la création)
clever env set --app "$APP_db" 'MYSQL_DATABASE' 'wordpress'
clever env set --app "$APP_db" 'MYSQL_PASSWORD' 'wordpress'
clever env set --app "$APP_db" 'MYSQL_ROOT_PASSWORD' 'somewordpress'
clever env set --app "$APP_db" 'MYSQL_USER' 'wordpress'
clever env set --app "$APP_wordpress" 'CC_COMPOSE_DNS_MAP' 'db='"${APP_db}.m.${NG_default}.cc-ng.cloud"
clever env set --app "$APP_wordpress" 'WORDPRESS_DB_HOST' "${APP_db}.m.${NG_default}.cc-ng.cloud"
clever env set --app "$APP_wordpress" 'WORDPRESS_DB_NAME' 'wordpress'
clever env set --app "$APP_wordpress" 'WORDPRESS_DB_PASSWORD' 'wordpress'
clever env set --app "$APP_wordpress" 'WORDPRESS_DB_USER' 'wordpress'

# Déploiements, dans l'ordre topologique (dépôts synthétiques exportés)
(
  cd repos/'db'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_db"
)
(
  cd repos/'wordpress'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_wordpress"
)
