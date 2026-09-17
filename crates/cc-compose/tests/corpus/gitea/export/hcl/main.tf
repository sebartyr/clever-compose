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
  name = "gitea-default"
  tags = ["compose-project=gitea", "compose-network=default", "compose-spec=9aa14d857c9c7c91984add8639f6f607"]
}

resource "clevercloud_postgresql" "db" {
  name          = "gitea-db"
  plan          = "xxs_sml"
  region        = "par"
  version       = "17"
  networkgroups = [
    {
      networkgroup_id = clevercloud_networkgroup.default.id
      fqdn            = "db.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    },
  ]
}

resource "clevercloud_docker" "gitea" {
  name               = "gitea-gitea"
  region             = "par"
  smallest_flavor    = "XS"
  biggest_flavor     = "XS"
  min_instance_count = 1
  max_instance_count = 1
  environment        = {
    CLEVER_COMPOSE_DNS_MAP = "db=${clevercloud_postgresql.db.id}.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    CLEVER_COMPOSE_EXPAND  = "DB_NAME,DB_USER,DB_PASSWD"
    DB_HOST                = "${clevercloud_postgresql.db.id}.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud:8080"
    DB_NAME                = "$${POSTGRESQL_ADDON_DB}"
    DB_PASSWD              = "$${POSTGRESQL_ADDON_PASSWORD}"
    DB_TYPE                = "postgres"
    DB_USER                = "$${POSTGRESQL_ADDON_USER}"
  }
  networkgroups      = [
    {
      networkgroup_id = clevercloud_networkgroup.default.id
      fqdn            = "gitea.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    },
  ]
  deployment {
    repository = var.repository
    commit     = "refs/heads/clever-compose/gitea"
  }
  dependencies       = [clevercloud_postgresql.db.id]
}
