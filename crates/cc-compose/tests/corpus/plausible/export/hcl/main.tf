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
  name = "plausible-default"
  tags = ["compose-project=plausible", "compose-network=default", "compose-spec=9f6f941066f236936479d8a2b1a7a52f"]
}

resource "clevercloud_postgresql" "plausible_db" {
  name          = "plausible-plausible_db"
  plan          = "xxs_sml"
  region        = "par"
  version       = "16"
  networkgroups = [
    {
      networkgroup_id = clevercloud_networkgroup.default.id
      fqdn            = "plausible_db.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    },
  ]
}

resource "clevercloud_docker" "plausible_events_db" {
  name               = "plausible-plausible_events_db"
  region             = "par"
  smallest_flavor    = "XS"
  biggest_flavor     = "XS"
  min_instance_count = 1
  max_instance_count = 1
  environment        = {
    CC_HEALTH_CHECK_PATH       = "/ping"
    CLICKHOUSE_SKIP_USER_SETUP = "1"
  }
  networkgroups      = [
    {
      networkgroup_id = clevercloud_networkgroup.default.id
      fqdn            = "plausible_events_db.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    },
  ]
  deployment {
    repository = var.repository
    commit     = "refs/heads/clever-compose/plausible_events_db"
  }
}

resource "clevercloud_docker" "plausible" {
  name               = "plausible-plausible"
  region             = "par"
  smallest_flavor    = "XS"
  biggest_flavor     = "XS"
  min_instance_count = 1
  max_instance_count = 1
  environment        = {
    BASE_URL                 = ""
    CLEVER_COMPOSE_DNS_MAP   = "plausible_db=${clevercloud_postgresql.plausible_db.id}.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud,plausible_events_db=${clevercloud_docker.plausible_events_db.id}.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    POSTGRESQL_ADDON_NG_HOST = "${clevercloud_postgresql.plausible_db.id}.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    POSTGRESQL_ADDON_NG_PORT = "8080"
    SECRET_KEY_BASE          = ""
    TMPDIR                   = "/var/lib/plausible/tmp"
  }
  networkgroups      = [
    {
      networkgroup_id = clevercloud_networkgroup.default.id
      fqdn            = "plausible.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    },
  ]
  deployment {
    repository = var.repository
    commit     = "refs/heads/clever-compose/plausible"
  }
  depends_on         = [clevercloud_docker.plausible_events_db]
  dependencies       = [clevercloud_postgresql.plausible_db.id]
}
