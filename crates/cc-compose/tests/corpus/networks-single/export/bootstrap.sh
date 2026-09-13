#!/usr/bin/env bash
# Bootstrap généré par clever-compose pour le projet `networks-single` (spec 3e7dfbe1271c4c6f31717dfd90d45ad1).
# À usage unique : ce script crée les ressources, il ne les réconcilie pas.
# Les dépôts synthétiques sont attendus dans ./repos/<service> (export).
set -euo pipefail
ORG="${CC_ORG:?organisation Clever Cloud requise (orga_xxx ou user_xxx)}"
cd "$(dirname "$0")"

# Network groups (un par réseau Compose)
clever features enable ng >/dev/null 2>&1 || true
clever ng create 'networks-single-default' -o "$ORG" --tags 'compose-project=networks-single,compose-network=default,compose-spec=3e7dfbe1271c4c6f31717dfd90d45ad1'
NG_default="$(clever ng get 'networks-single-default' -o "$ORG" -F json | jq -r .id)"

# Applications, créées sans déploiement
APP_web="$(clever create -t docker 'networks-single-web' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_web" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=networks-single
# tag : compose-service=web
# tag : compose-spec=3e7dfbe1271c4c6f31717dfd90d45ad1
APP_api="$(clever create -t docker 'networks-single-api' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_api" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=networks-single
# tag : compose-service=api
# tag : compose-spec=3e7dfbe1271c4c6f31717dfd90d45ad1

# Liaisons aux network groups
clever ng link "$APP_web" 'networks-single-default' -o "$ORG"
clever ng link "$APP_api" 'networks-single-default' -o "$ORG"

# Variables d'environnement (les FQDN des membres sont connus dès la création)
clever env set --app "$APP_web" 'UPSTREAM' 'http://api:3000'

# Déploiements, dans l'ordre topologique (dépôts synthétiques exportés)
(
  cd repos/'web'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_web"
)
(
  cd repos/'api'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_api"
)
