terraform {
  required_providers {
    clevercloud = { source = "CleverCloud/clevercloud", version = "~> 2.1" }
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
  description = "Dépôt git qui héberge les branches clever-compose/<service> (dépôts synthétiques poussés par `clever-compose export`)"
}

resource "clevercloud_networkgroup" "default" {
  name = "redirections-default"
  tags = ["compose-project=redirections", "compose-network=default", "compose-spec=ab46b6e9e5851eda9d6f09c939d555f5"]
}

resource "clevercloud_docker" "db" {
  name               = "redirections-db"
  region             = "par"
  smallest_flavor    = "XS"
  biggest_flavor     = "XS"
  min_instance_count = 1
  max_instance_count = 1
  environment        = {
    POSTGRES_PASSWORD = "secret"
  }
  networkgroups      = [
    {
      networkgroup_id = clevercloud_networkgroup.default.id
      fqdn            = "db.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    },
  ]
  redirection        = {
    namespace = "default"
  }
  deployment {
    repository = var.repository
    commit     = "refs/heads/clever-compose/db"
  }
}

resource "clevercloud_docker" "game" {
  name               = "redirections-game"
  region             = "par"
  smallest_flavor    = "XS"
  biggest_flavor     = "XS"
  min_instance_count = 1
  max_instance_count = 1
  networkgroups      = [
    {
      networkgroup_id = clevercloud_networkgroup.default.id
      fqdn            = "game.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    },
  ]
  redirection        = {
    namespace = "cleverapps"
  }
  deployment {
    repository = var.repository
    commit     = "refs/heads/clever-compose/game"
  }
}
