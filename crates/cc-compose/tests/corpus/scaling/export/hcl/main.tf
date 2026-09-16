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
  name = "scaling-default"
  tags = ["compose-project=scaling", "compose-network=default", "compose-spec=7ed1aecaf191a71f6301899140956ee5"]
}

resource "clevercloud_docker" "fixed" {
  name               = "scaling-fixed"
  region             = "par"
  smallest_flavor    = "M"
  biggest_flavor     = "M"
  min_instance_count = 3
  max_instance_count = 3
  networkgroups      = [
    {
      networkgroup_id = clevercloud_networkgroup.default.id
      fqdn            = "fixed.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    },
  ]
  deployment {
    repository = var.repository
    commit     = "refs/heads/clever-compose/fixed"
  }
}

resource "clevercloud_docker" "auto" {
  name               = "scaling-auto"
  region             = "par"
  smallest_flavor    = "S"
  biggest_flavor     = "L"
  min_instance_count = 1
  max_instance_count = 4
  networkgroups      = [
    {
      networkgroup_id = clevercloud_networkgroup.default.id
      fqdn            = "auto.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    },
  ]
  deployment {
    repository = var.repository
    commit     = "refs/heads/clever-compose/auto"
  }
}

resource "clevercloud_docker" "capped" {
  name               = "scaling-capped"
  region             = "par"
  smallest_flavor    = "3XL"
  biggest_flavor     = "3XL"
  min_instance_count = 1
  max_instance_count = 1
  networkgroups      = [
    {
      networkgroup_id = clevercloud_networkgroup.default.id
      fqdn            = "capped.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    },
  ]
  deployment {
    repository = var.repository
    commit     = "refs/heads/clever-compose/capped"
  }
}
