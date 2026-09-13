#!/usr/bin/env bash
# Bootstrap généré par clever-compose pour le projet `gitea` (spec 68a37ba677359e09018f300954276e41).
# À usage unique : ce script crée les ressources, il ne les réconcilie pas.
# Les dépôts synthétiques sont attendus dans ./repos/<service> (export).
set -euo pipefail
ORG="${CC_ORG:?organisation Clever Cloud requise (orga_xxx ou user_xxx)}"
cd "$(dirname "$0")"

# Network groups (un par réseau Compose)
clever features enable ng >/dev/null 2>&1 || true
clever ng create 'gitea-default' -o "$ORG" --tags 'compose-project=gitea,compose-network=default,compose-spec=68a37ba677359e09018f300954276e41'
NG_default="$(clever ng get 'gitea-default' -o "$ORG" -F json | jq -r .id)"

# Applications, créées sans déploiement
APP_db="$(clever create -t docker 'gitea-db' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_db" --flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=gitea
# tag : compose-service=db
# tag : compose-spec=68a37ba677359e09018f300954276e41
APP_gitea="$(clever create -t docker 'gitea-gitea' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_gitea" --flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=gitea
# tag : compose-service=gitea
# tag : compose-spec=68a37ba677359e09018f300954276e41

# Liaisons aux network groups
clever ng link "$APP_db" 'gitea-default' -o "$ORG"
clever ng link "$APP_gitea" 'gitea-default' -o "$ORG"

# Variables d'environnement (les FQDN des membres sont connus dès la création)
clever env set --app "$APP_db" 'POSTGRES_DB' 'gitea'
clever env set --app "$APP_db" 'POSTGRES_PASSWORD' 'gitea'
clever env set --app "$APP_db" 'POSTGRES_USER' 'gitea'
clever env set --app "$APP_gitea" 'CC_COMPOSE_DNS_MAP' 'db='"${APP_db}.m.${NG_default}.cc-ng.cloud"
clever env set --app "$APP_gitea" 'DB_HOST' "${APP_db}.m.${NG_default}.cc-ng.cloud"':5432'
clever env set --app "$APP_gitea" 'DB_NAME' 'gitea'
clever env set --app "$APP_gitea" 'DB_PASSWD' 'gitea'
clever env set --app "$APP_gitea" 'DB_TYPE' 'postgres'
clever env set --app "$APP_gitea" 'DB_USER' 'gitea'

# Déploiements, dans l'ordre topologique (dépôts synthétiques exportés)
(
  cd repos/'db'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_db"
)
(
  cd repos/'gitea'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_gitea"
)
