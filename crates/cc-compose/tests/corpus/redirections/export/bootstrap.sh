#!/usr/bin/env bash
# Bootstrap généré par clever-compose pour le projet `redirections` (spec cdf5f26b02357c05b8a4da821fe3451e).
# À usage unique : ce script crée les ressources, il ne les réconcilie pas.
# Les dépôts synthétiques sont attendus dans ./repos/<service> (export).
set -euo pipefail
ORG="${CC_ORG:?organisation Clever Cloud requise (orga_xxx ou user_xxx)}"
cd "$(dirname "$0")"

# Network groups (un par réseau Compose)
clever features enable ng >/dev/null 2>&1 || true
clever ng create 'redirections-default' -o "$ORG" --tags 'compose-project=redirections,compose-network=default,compose-spec=cdf5f26b02357c05b8a4da821fe3451e'
NG_default="$(clever ng get 'redirections-default' -o "$ORG" -F json | jq -r .id)"

# Applications, créées sans déploiement
APP_db="$(clever create -t docker 'redirections-db' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_db" --flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=redirections
# tag : compose-service=db
# tag : compose-spec=cdf5f26b02357c05b8a4da821fe3451e
APP_game="$(clever create -t docker 'redirections-game' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_game" --flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=redirections
# tag : compose-service=game
# tag : compose-spec=cdf5f26b02357c05b8a4da821fe3451e

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
( cd repos/'db' && git init -q -b main 2>/dev/null || true && git add -A && git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q -m 'clever-compose' || true && clever deploy --app "$APP_db" )
( cd repos/'game' && git init -q -b main 2>/dev/null || true && git add -A && git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q -m 'clever-compose' || true && clever deploy --app "$APP_game" )
