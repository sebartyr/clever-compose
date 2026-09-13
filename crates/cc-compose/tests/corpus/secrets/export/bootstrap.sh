#!/usr/bin/env bash
# Bootstrap généré par clever-compose pour le projet `secrets` (spec 146d3418d2188ff3c87af2b66be889bc).
# À usage unique : ce script crée les ressources, il ne les réconcilie pas.
# Les dépôts synthétiques sont attendus dans ./repos/<service> (export).
set -euo pipefail
ORG="${CC_ORG:?organisation Clever Cloud requise (orga_xxx ou user_xxx)}"
cd "$(dirname "$0")"

# Network groups (un par réseau Compose)
clever features enable ng >/dev/null 2>&1 || true
clever ng create 'secrets-default' -o "$ORG" --tags 'compose-project=secrets,compose-network=default,compose-spec=146d3418d2188ff3c87af2b66be889bc'
NG_default="$(clever ng get 'secrets-default' -o "$ORG" -F json | jq -r .id)"

# Applications, créées sans déploiement
APP_app="$(clever create -t docker 'secrets-app' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_app" --flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=secrets
# tag : compose-service=app
# tag : compose-spec=146d3418d2188ff3c87af2b66be889bc

# Liaisons aux network groups
clever ng link "$APP_app" 'secrets-default' -o "$ORG"

# Variables d'environnement (les FQDN des membres sont connus dès la création)
clever env set --app "$APP_app" 'CC_COMPOSE_SECRET_API_TOKEN' "${API_TOKEN:?variable API_TOKEN requise pour le secret api_token}"
clever env set --app "$APP_app" 'CC_COMPOSE_SECRET_DB_PASSWORD' "$(cat '../db_password.txt')"
clever env set --app "$APP_app" 'DB_PASSWORD_FILE' '/run/secrets/db_password'

# Déploiements, dans l'ordre topologique (dépôts synthétiques exportés)
(
  cd repos/'app'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_app"
)
