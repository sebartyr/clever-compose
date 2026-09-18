terraform {
  required_providers {
    clevercloud = { source = "CleverCloud/clevercloud", version = "~> 2.2" }
  }
}

provider "clevercloud" {
  organisation = var.organisation
}

variable "organisation" {
  type        = string
  description = "Organisation Clever Cloud (orga_xxx ou user_xxx)"
  sensitive   = true
}

variable "repository" {
  type        = string
  description = "Dépôt git qui porte les branches clever-compose/<service> (écrites par `clever-compose export --branches`) : URL distante, ou `file:///chemin/local` (ouvert sur place par le provider)"
}

resource "clevercloud_networkgroup" "default" {
  name = "cellar-default"
  tags = ["compose-project=cellar", "compose-network=default", "compose-spec=69a56c6d4bb8e54a150822a30e3bf3b2"]
}

resource "clevercloud_postgresql" "db" {
  name          = "cellar-db"
  plan          = "xxs_sml"
  region        = "par"
  version       = "18"
  networkgroups = [
    {
      networkgroup_id = clevercloud_networkgroup.default.id
      fqdn            = "db.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    },
  ]
}

resource "clevercloud_cellar" "files" {
  name = "cellar-files"
}

resource "clevercloud_cellar_bucket" "files_vikunja_files" {
  id        = "vikunja-files"
  cellar_id = clevercloud_cellar.files.id
}

resource "clevercloud_docker" "vikunja" {
  name               = "cellar-vikunja"
  region             = "par"
  smallest_flavor    = "XS"
  biggest_flavor     = "XS"
  min_instance_count = 1
  max_instance_count = 1
  environment        = {
    CELLAR_ADDON_BUCKET           = "vikunja-files"
    CLEVER_COMPOSE_DNS_MAP        = "db=${clevercloud_postgresql.db.id}.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    CLEVER_COMPOSE_EXPAND         = "VIKUNJA_FILES_S3_ENDPOINT,VIKUNJA_FILES_S3_ACCESSKEY,VIKUNJA_FILES_S3_SECRETKEY,VIKUNJA_DATABASE_PASSWORD,VIKUNJA_DATABASE_USER,VIKUNJA_DATABASE_DATABASE"
    POSTGRESQL_ADDON_NG_HOST      = "${clevercloud_postgresql.db.id}.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    POSTGRESQL_ADDON_NG_PORT      = "8080"
    VIKUNJA_DATABASE_DATABASE     = "$${POSTGRESQL_ADDON_DB}"
    VIKUNJA_DATABASE_HOST         = "${clevercloud_postgresql.db.id}.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud:8080"
    VIKUNJA_DATABASE_PASSWORD     = "$${POSTGRESQL_ADDON_PASSWORD}"
    VIKUNJA_DATABASE_TYPE         = "postgres"
    VIKUNJA_DATABASE_USER         = "$${POSTGRESQL_ADDON_USER}"
    VIKUNJA_FILES_S3_ACCESSKEY    = "$${CELLAR_ADDON_KEY_ID}"
    VIKUNJA_FILES_S3_BUCKET       = "vikunja-files"
    VIKUNJA_FILES_S3_ENDPOINT     = "https://$${CELLAR_ADDON_HOST}"
    VIKUNJA_FILES_S3_REGION       = "us-east-1"
    VIKUNJA_FILES_S3_SECRETKEY    = "$${CELLAR_ADDON_KEY_SECRET}"
    VIKUNJA_FILES_S3_USEPATHSTYLE = "true"
    VIKUNJA_FILES_TYPE            = "s3"
  }
  networkgroups      = [
    {
      networkgroup_id = clevercloud_networkgroup.default.id
      fqdn            = "vikunja.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    },
  ]
  deployment {
    repository = var.repository
    commit     = "refs/heads/clever-compose/vikunja"
  }
  dependencies       = [clevercloud_postgresql.db.id, clevercloud_cellar.files.id]
}
