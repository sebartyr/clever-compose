#!/usr/bin/env bash
# Bootstrap généré par clever-compose pour le projet `redirections` (spec ab46b6e9e5851eda9d6f09c939d555f5).
# À usage unique : ce script crée les ressources, il ne les réconcilie pas.
# Les dépôts synthétiques sont attendus dans ./repos/<service> (export).
set -euo pipefail
ORG="${CC_ORG:?organisation Clever Cloud requise (orga_xxx ou user_xxx)}"
cd "$(dirname "$0")"

# Network groups (un par réseau Compose)
clever features enable ng >/dev/null 2>&1 || true
clever ng create 'redirections-default' -o "$ORG" --tags 'compose-project=redirections,compose-network=default,compose-spec=ab46b6e9e5851eda9d6f09c939d555f5'
NG_default="$(clever ng get 'redirections-default' -o "$ORG" -F json | jq -r .id)"

# Applications, créées sans déploiement
APP_db="$(clever create -t docker 'redirections-db' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_db" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=redirections
# tag : compose-service=db
# tag : compose-spec=ab46b6e9e5851eda9d6f09c939d555f5
APP_game="$(clever create -t docker 'redirections-game' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_game" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=redirections
# tag : compose-service=game
# tag : compose-spec=ab46b6e9e5851eda9d6f09c939d555f5

# Liaisons aux network groups
clever ng link "$APP_db" 'redirections-default' -o "$ORG"
clever ng link "$APP_game" 'redirections-default' -o "$ORG"

# Variables d'environnement (les FQDN des membres sont connus dès la création)
clever env set --app "$APP_db" 'POSTGRES_PASSWORD' 'secret'

# Redirections (port externe attribué par la plateforme, affiché par la commande)
clever tcp-redirs add --app "$APP_db" --namespace 'default'
# UDP 27015 : pas de commande clever-tools ; API POST .../applications/$APP_game/udpRedirs
clever tcp-redirs add --app "$APP_game" --namespace 'cleverapps'

# Déploiements, dans l'ordre topologique (dépôts synthétiques exportés)
(
  cd repos/'db'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_db"
)
(
  cd repos/'game'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_game"
)
