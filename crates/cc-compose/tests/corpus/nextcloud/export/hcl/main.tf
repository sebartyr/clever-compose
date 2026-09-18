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
  name = "nextcloud-default"
  tags = ["compose-project=nextcloud", "compose-network=default", "compose-spec=a8756149457c4f7cfba9e5a2388e71c4"]
}

resource "clevercloud_postgresql" "db" {
  name          = "nextcloud-db"
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

resource "clevercloud_docker" "nc" {
  name               = "nextcloud-nc"
  region             = "par"
  smallest_flavor    = "XS"
  biggest_flavor     = "XS"
  min_instance_count = 1
  max_instance_count = 1
  environment        = {
    CLEVER_COMPOSE_DNS_MAP   = "db=${clevercloud_postgresql.db.id}.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    CLEVER_COMPOSE_EXPAND    = "POSTGRES_PASSWORD,POSTGRES_DB,POSTGRES_USER"
    POSTGRESQL_ADDON_NG_HOST = "${clevercloud_postgresql.db.id}.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    POSTGRESQL_ADDON_NG_PORT = "8080"
    POSTGRES_DB              = "$${POSTGRESQL_ADDON_DB}"
    POSTGRES_HOST            = "${clevercloud_postgresql.db.id}.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    POSTGRES_PASSWORD        = "$${POSTGRESQL_ADDON_PASSWORD}"
    POSTGRES_USER            = "$${POSTGRESQL_ADDON_USER}"
  }
  networkgroups      = [
    {
      networkgroup_id = clevercloud_networkgroup.default.id
      fqdn            = "nc.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    },
  ]
  deployment {
    repository = var.repository
    commit     = "refs/heads/clever-compose/nc"
  }
  dependencies       = [clevercloud_postgresql.db.id]
}
