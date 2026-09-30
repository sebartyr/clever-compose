#!/usr/bin/env bash
# Bootstrap généré par clever-compose pour le projet `build-flavor` (spec 98fcf00120eb071014f6e68a4581acd7).
# À usage unique : ce script crée les ressources, il ne les réconcilie pas.
# Les dépôts synthétiques sont attendus dans ./repos/<service> (export).
set -euo pipefail
ORG="${CLEVER_COMPOSE_ORG:?organisation Clever Cloud requise (orga_xxx ou user_xxx)}"
cd "$(dirname "$0")"

# Network groups (un par réseau Compose)
clever features enable ng >/dev/null 2>&1 || true
clever ng create 'build-flavor-default' -o "$ORG" --tags 'compose-project=build-flavor,compose-network=default,compose-spec=98fcf00120eb071014f6e68a4581acd7'
NG_default="$(clever ng get 'build-flavor-default' -o "$ORG" -F json | jq -r .id)"

# Applications, créées sans déploiement
APP_plain="$(clever create -t docker 'build-flavor-plain' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_plain" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1 --build-flavor 'M'
# tag : compose-project=build-flavor
# tag : compose-service=plain
# tag : compose-spec=98fcf00120eb071014f6e68a4581acd7
APP_big="$(clever create -t docker 'build-flavor-big' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_big" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1 --build-flavor 'XL'
# tag : compose-project=build-flavor
# tag : compose-service=big
# tag : compose-spec=98fcf00120eb071014f6e68a4581acd7
APP_inline="$(clever create -t docker 'build-flavor-inline' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_inline" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=build-flavor
# tag : compose-service=inline
# tag : compose-spec=98fcf00120eb071014f6e68a4581acd7

# Liaisons aux network groups
clever ng link "$APP_plain" 'build-flavor-default' -o "$ORG"
clever ng link "$APP_big" 'build-flavor-default' -o "$ORG"
clever ng link "$APP_inline" 'build-flavor-default' -o "$ORG"

# Variables d'environnement (les FQDN des membres sont connus dès la création)

# Déploiements, dans l'ordre topologique (dépôts synthétiques exportés)
(
  cd repos/'plain'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_plain"
)
(
  cd repos/'big'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_big"
)
(
  cd repos/'inline'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_inline"
)
