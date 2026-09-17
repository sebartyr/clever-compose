#!/usr/bin/env bash
# Bootstrap généré par clever-compose pour le projet `nginx-proxy` (spec da5189ef58842a94fc072d48ba2e55e6).
# À usage unique : ce script crée les ressources, il ne les réconcilie pas.
# Les dépôts synthétiques sont attendus dans ./repos/<service> (export).
set -euo pipefail
ORG="${CLEVER_COMPOSE_ORG:?organisation Clever Cloud requise (orga_xxx ou user_xxx)}"
cd "$(dirname "$0")"

# Network groups (un par réseau Compose)
clever features enable ng >/dev/null 2>&1 || true
clever ng create 'nginx-proxy-default' -o "$ORG" --tags 'compose-project=nginx-proxy,compose-network=default,compose-spec=da5189ef58842a94fc072d48ba2e55e6'
NG_default="$(clever ng get 'nginx-proxy-default' -o "$ORG" -F json | jq -r .id)"

# Applications, créées sans déploiement
APP_app="$(clever create -t docker 'nginx-proxy-app' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_app" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=nginx-proxy
# tag : compose-service=app
# tag : compose-spec=da5189ef58842a94fc072d48ba2e55e6
APP_front="$(clever create -t docker 'nginx-proxy-front' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_front" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=nginx-proxy
# tag : compose-service=front
# tag : compose-spec=da5189ef58842a94fc072d48ba2e55e6

# Liaisons aux network groups
clever ng link "$APP_front" 'nginx-proxy-default' -o "$ORG"
clever ng link "$APP_app" 'nginx-proxy-default' -o "$ORG"

# Variables d'environnement (les FQDN des membres sont connus dès la création)
clever env set --app "$APP_front" 'CLEVER_COMPOSE_DNS_MAP' 'app='"${APP_app}.m.${NG_default}.cc-ng.cloud"

# Déploiements, dans l'ordre topologique (dépôts synthétiques exportés)
(
  cd repos/'app'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_app"
)
(
  cd repos/'front'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_front"
)
