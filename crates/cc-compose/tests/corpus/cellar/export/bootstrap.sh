#!/usr/bin/env bash
# Bootstrap généré par clever-compose pour le projet `cellar` (spec 69a56c6d4bb8e54a150822a30e3bf3b2).
# À usage unique : ce script crée les ressources, il ne les réconcilie pas.
# Les dépôts synthétiques sont attendus dans ./repos/<service> (export).
set -euo pipefail
ORG="${CLEVER_COMPOSE_ORG:?organisation Clever Cloud requise (orga_xxx ou user_xxx)}"
cd "$(dirname "$0")"

# Network groups (un par réseau Compose)
clever features enable ng >/dev/null 2>&1 || true
clever ng create 'cellar-default' -o "$ORG" --tags 'compose-project=cellar,compose-network=default,compose-spec=69a56c6d4bb8e54a150822a30e3bf3b2'
NG_default="$(clever ng get 'cellar-default' -o "$ORG" -F json | jq -r .id)"

# Addons managés (facturés selon le plan)
clever addon create 'postgresql-addon' 'cellar-db' --plan 'xxs_sml' --region 'par' --addon-version '18' -o "$ORG"
ADDON_db="$(clever addon list -o "$ORG" -F json | jq -r '.[] | select(.name == "cellar-db") | .addonId')"
ADDON_REAL_db="$(clever addon list -o "$ORG" -F json | jq -r '.[] | select(.name == "cellar-db") | .realId')"
clever addon create 'cellar-addon' 'cellar-files' --plan 'S' --region 'par' -o "$ORG"
ADDON_files="$(clever addon list -o "$ORG" -F json | jq -r '.[] | select(.name == "cellar-files") | .addonId')"
eval "$(clever addon env "$ADDON_files" -o "$ORG" -F shell)"
s3cmd --host "$CELLAR_ADDON_HOST" --host-bucket "$CELLAR_ADDON_HOST" --access_key "$CELLAR_ADDON_KEY_ID" --secret_key "$CELLAR_ADDON_KEY_SECRET" mb 's3://vikunja-files' || true  # existant : sans effet

# Applications, créées sans déploiement
APP_vikunja="$(clever create -t docker 'cellar-vikunja' -o "$ORG" -r 'par' -F json | jq -r .id)"
clever scale --app "$APP_vikunja" --min-flavor 'XS' --max-flavor 'XS' --min-instances 1 --max-instances 1
# tag : compose-project=cellar
# tag : compose-service=vikunja
# tag : compose-spec=69a56c6d4bb8e54a150822a30e3bf3b2

# Liaisons aux network groups
clever ng link "$APP_vikunja" 'cellar-default' -o "$ORG"
clever ng link "$ADDON_REAL_db" 'cellar-default' -o "$ORG"

# Liaisons aux addons (injection de leurs variables)
clever service link-addon "$ADDON_db" --app "$APP_vikunja"
clever service link-addon "$ADDON_files" --app "$APP_vikunja"

# Variables d'environnement (les FQDN des membres sont connus dès la création)
clever env set --app "$APP_vikunja" 'CELLAR_ADDON_BUCKET' 'vikunja-files'
clever env set --app "$APP_vikunja" 'CLEVER_COMPOSE_DNS_MAP' 'db='"${ADDON_REAL_db}.m.${NG_default}.cc-ng.cloud"
clever env set --app "$APP_vikunja" 'CLEVER_COMPOSE_EXPAND' 'VIKUNJA_FILES_S3_ENDPOINT,VIKUNJA_FILES_S3_ACCESSKEY,VIKUNJA_FILES_S3_SECRETKEY,VIKUNJA_DATABASE_PASSWORD,VIKUNJA_DATABASE_USER,VIKUNJA_DATABASE_DATABASE'
clever env set --app "$APP_vikunja" 'POSTGRESQL_ADDON_NG_HOST' "${ADDON_REAL_db}.m.${NG_default}.cc-ng.cloud"
clever env set --app "$APP_vikunja" 'POSTGRESQL_ADDON_NG_PORT' '8080'
clever env set --app "$APP_vikunja" 'VIKUNJA_DATABASE_DATABASE' '${POSTGRESQL_ADDON_DB}'
clever env set --app "$APP_vikunja" 'VIKUNJA_DATABASE_HOST' "${ADDON_REAL_db}.m.${NG_default}.cc-ng.cloud"':8080'
clever env set --app "$APP_vikunja" 'VIKUNJA_DATABASE_PASSWORD' '${POSTGRESQL_ADDON_PASSWORD}'
clever env set --app "$APP_vikunja" 'VIKUNJA_DATABASE_TYPE' 'postgres'
clever env set --app "$APP_vikunja" 'VIKUNJA_DATABASE_USER' '${POSTGRESQL_ADDON_USER}'
clever env set --app "$APP_vikunja" 'VIKUNJA_FILES_S3_ACCESSKEY' '${CELLAR_ADDON_KEY_ID}'
clever env set --app "$APP_vikunja" 'VIKUNJA_FILES_S3_BUCKET' 'vikunja-files'
clever env set --app "$APP_vikunja" 'VIKUNJA_FILES_S3_ENDPOINT' 'https://''${CELLAR_ADDON_HOST}'
clever env set --app "$APP_vikunja" 'VIKUNJA_FILES_S3_REGION' 'us-east-1'
clever env set --app "$APP_vikunja" 'VIKUNJA_FILES_S3_SECRETKEY' '${CELLAR_ADDON_KEY_SECRET}'
clever env set --app "$APP_vikunja" 'VIKUNJA_FILES_S3_USEPATHSTYLE' 'true'
clever env set --app "$APP_vikunja" 'VIKUNJA_FILES_TYPE' 's3'

# Déploiements, dans l'ordre topologique (dépôts synthétiques exportés)
(
  cd repos/'vikunja'
  [ -d .git ] || git init -q -b main
  git add -A
  git -c user.name=clever-compose -c user.email=clever-compose@localhost commit -q --allow-empty -m 'clever-compose'
  clever deploy --app "$APP_vikunja"
)
