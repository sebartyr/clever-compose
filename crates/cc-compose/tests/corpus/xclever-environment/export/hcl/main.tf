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
  name = "xclever-environment-default"
  tags = ["compose-project=xclever-environment", "compose-network=default", "compose-spec=fbcfa2da4c410c866c34c5f0131504d5"]
}

resource "clevercloud_postgresql" "db1" {
  name          = "xclever-environment-db1"
  plan          = "xxs_sml"
  region        = "par"
  version       = "18"
  networkgroups = [
    {
      networkgroup_id = clevercloud_networkgroup.default.id
      fqdn            = "db1.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    },
  ]
}

resource "clevercloud_postgresql" "db2" {
  name          = "xclever-environment-db2"
  plan          = "xxs_sml"
  region        = "par"
  version       = "18"
  networkgroups = [
    {
      networkgroup_id = clevercloud_networkgroup.default.id
      fqdn            = "db2.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    },
  ]
}

resource "clevercloud_docker" "app" {
  name               = "xclever-environment-app"
  region             = "par"
  smallest_flavor    = "XS"
  biggest_flavor     = "XS"
  min_instance_count = 1
  max_instance_count = 1
  environment        = {
    APP_DB_HOST            = "${clevercloud_postgresql.db2.id}.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud:8080"
    APP_DB_PASSWORD        = "$${POSTGRESQL_ADDON_PASSWORD}"
    APP_DB_URL             = "postgres://$${POSTGRESQL_ADDON_USER}:$${POSTGRESQL_ADDON_PASSWORD}@${clevercloud_postgresql.db2.id}.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud:8080/$${POSTGRESQL_ADDON_DB}?sslmode=disable"
    APP_DB_USER            = "$${POSTGRESQL_ADDON_USER}"
    APP_SECRET             = "literal-sur-clever"
    CLEVER_COMPOSE_DNS_MAP = "db1=${clevercloud_postgresql.db1.id}.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud,db2=${clevercloud_postgresql.db2.id}.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    CLEVER_COMPOSE_EXPAND  = "APP_DB_PASSWORD,APP_DB_USER,APP_DB_URL"
  }
  networkgroups      = [
    {
      networkgroup_id = clevercloud_networkgroup.default.id
      fqdn            = "app.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    },
  ]
  deployment {
    repository = var.repository
    commit     = "refs/heads/clever-compose/app"
  }
  dependencies       = [clevercloud_postgresql.db1.id, clevercloud_postgresql.db2.id]
}
