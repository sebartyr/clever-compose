#!/usr/bin/env bash
# Bootstrap généré par clever-compose pour le projet `scaling` (spec 7ed1aecaf191a71f6301899140956ee5).
# À usage unique : ce script crée les ressources, il ne les réconcilie pas.
# Les dépôts synthétiques sont attendus dans ./repos/<service> (export).
set -euo pipefail
ORG="${CC_ORG:?organisation Clever Cloud requise (orga_xxx ou user_xxx)}"
cd "$(dirname "$0")"

# Network groups (un par réseau Compose)
clever features enable ng >/dev/null 2>&1 || true
clever ng create 'scaling-default' -o "$ORG" --tags 'compose-project=scaling,compose-network=default,compose-spec=7ed1aecaf191a71f6301899140956ee5'
NG_default="$(clever ng get 'scaling-default' -o "$ORG" -F json | jq -r .id)"

# Applications, créées sans déploiement
APP_fixed="$(clever create -t docker 'scaling-fixed' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_fixed" --min-flavor 'M' --max-flavor 'M' --min-instances 3 --max-instances 3
# tag : compose-project=scaling
# tag : compose-service=fixed
# tag : compose-spec=7ed1aecaf191a71f6301899140956ee5
APP_auto="$(clever create -t docker 'scaling-auto' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_auto" --min-flavor 'S' --max-flavor 'L' --min-instances 1 --max-instances 4
# tag : compose-project=scaling
# tag : compose-service=auto
# tag : compose-spec=7ed1aecaf191a71f6301899140956ee5
APP_capped="$(clever create -t docker 'scaling-capped' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_capped" --min-flavor '3XL' --max-flavor '3XL' --min-instances 1 --max-instances 1
# tag : compose-project=scaling
# tag : compose-service=capped
# tag : compose-spec=7ed1aecaf191a71f6301899140956ee5

# Liaisons aux network groups
clever ng link "$APP_fixed" 'scaling-default' -o "$ORG"
clever ng link "$APP_auto" 'scaling-default' -o "$ORG"
clever ng link "$APP_capped" 'scaling-default' -o "$ORG"

# Variables d'environnement (les FQDN des membres sont connus dès la création)

# Déploiements, dans l'ordre topologique (dépôts synthétiques exportés)
(
  cd repos/'fixed'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_fixed"
)
(
  cd repos/'auto'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_auto"
)
(
  cd repos/'capped'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_capped"
)
