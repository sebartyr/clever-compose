#!/usr/bin/env bash
# Bootstrap généré par clever-compose pour le projet `wordpress-addon` (spec a964eb627f2fac67e8cbb8509eb6ad4c).
# À usage unique : ce script crée les ressources, il ne les réconcilie pas.
# Les dépôts synthétiques sont attendus dans ./repos/<service> (export).
set -euo pipefail
ORG="${CC_ORG:?organisation Clever Cloud requise (orga_xxx ou user_xxx)}"
cd "$(dirname "$0")"

# Network groups (un par réseau Compose)
clever features enable ng >/dev/null 2>&1 || true
clever ng create 'wordpress-addon-default' -o "$ORG" --tags 'compose-project=wordpress-addon,compose-network=default,compose-spec=a964eb627f2fac67e8cbb8509eb6ad4c'
NG_default="$(clever ng get 'wordpress-addon-default' -o "$ORG" -F json | jq -r .id)"

# Addons managés (facturés selon le plan)
clever addon create 'mysql-addon' 'wordpress-addon-db' --plan 'dev' --region 'par' --version '8.4' -o "$ORG"
ADDON_db="$(clever addon list -o "$ORG" -F json | jq -r '.[] | select(.name == "wordpress-addon-db") | .addonId')"

# Applications, créées sans déploiement
APP_wordpress="$(clever create -t docker 'wordpress-addon-wordpress' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_wordpress" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=wordpress-addon
# tag : compose-service=wordpress
# tag : compose-spec=a964eb627f2fac67e8cbb8509eb6ad4c

# Liaisons aux network groups
clever ng link "$APP_wordpress" 'wordpress-addon-default' -o "$ORG"

# Liaisons aux addons (injection de leurs variables)
clever service link-addon "$ADDON_db" --app "$APP_wordpress"

# Variables d'environnement (les FQDN des membres sont connus dès la création)
clever env set --app "$APP_wordpress" 'CC_COMPOSE_EXPAND' 'WORDPRESS_DB_HOST,WORDPRESS_DB_USER,WORDPRESS_DB_PASSWORD,WORDPRESS_DB_NAME'
clever env set --app "$APP_wordpress" 'WORDPRESS_DB_HOST' '${MYSQL_ADDON_HOST}'
clever env set --app "$APP_wordpress" 'WORDPRESS_DB_NAME' '${MYSQL_ADDON_DB}'
clever env set --app "$APP_wordpress" 'WORDPRESS_DB_PASSWORD' '${MYSQL_ADDON_PASSWORD}'
clever env set --app "$APP_wordpress" 'WORDPRESS_DB_USER' '${MYSQL_ADDON_USER}'

# Déploiements, dans l'ordre topologique (dépôts synthétiques exportés)
(
  cd repos/'wordpress'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_wordpress"
)
