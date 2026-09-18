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
  name = "init-scripts-default"
  tags = ["compose-project=init-scripts", "compose-network=default", "compose-spec=9fbb301c3c1c91de1fe99c5ae8acc5b9"]
}

resource "clevercloud_postgresql" "db" {
  name          = "init-scripts-db"
  plan          = "xxs_sml"
  region        = "par"
  version       = "15"
  networkgroups = [
    {
      networkgroup_id = clevercloud_networkgroup.default.id
      fqdn            = "db.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    },
  ]
}

resource "clevercloud_docker" "db_init" {
  name               = "init-scripts-db-init"
  region             = "par"
  smallest_flavor    = "XS"
  biggest_flavor     = "XS"
  min_instance_count = 1
  max_instance_count = 1
  environment        = {
    CLEVER_COMPOSE_INIT_HASH = "58d214e86fa675d00c5603cc537d6ae5"
  }
  deployment {
    repository = var.repository
    commit     = "refs/heads/clever-compose/db-init"
  }
  dependencies       = [clevercloud_postgresql.db.id]
}

resource "clevercloud_docker" "api" {
  name               = "init-scripts-api"
  region             = "par"
  smallest_flavor    = "XS"
  biggest_flavor     = "XS"
  min_instance_count = 1
  max_instance_count = 1
  environment        = {
    CLEVER_COMPOSE_DNS_MAP   = "db=${clevercloud_postgresql.db.id}.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    CLEVER_COMPOSE_EXPAND    = "DATABASE_URL,DB_PASSWORD"
    DATABASE_URL             = "postgres://$${POSTGRESQL_ADDON_USER}:$${POSTGRESQL_ADDON_PASSWORD}@${clevercloud_postgresql.db.id}.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud:8080/$${POSTGRESQL_ADDON_DB}?sslmode=disable"
    DB_PASSWORD              = "$${POSTGRESQL_ADDON_PASSWORD}"
    POSTGRESQL_ADDON_NG_HOST = "${clevercloud_postgresql.db.id}.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    POSTGRESQL_ADDON_NG_PORT = "8080"
  }
  networkgroups      = [
    {
      networkgroup_id = clevercloud_networkgroup.default.id
      fqdn            = "api.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    },
  ]
  deployment {
    repository = var.repository
    commit     = "refs/heads/clever-compose/api"
  }
  depends_on         = [clevercloud_docker.db_init]
  dependencies       = [clevercloud_postgresql.db.id]
}
