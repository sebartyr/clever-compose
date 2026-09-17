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

variable "secret_api_token" {
  type        = string
  sensitive   = true
  description = "Valeur du secret `api_token` (variable d'environnement API_TOKEN au moment du up)"
}

resource "clevercloud_networkgroup" "default" {
  name = "secrets-default"
  tags = ["compose-project=secrets", "compose-network=default", "compose-spec=69b84dbca4ba8bebc96faaaea68b4416"]
}

resource "clevercloud_docker" "app" {
  name               = "secrets-app"
  region             = "par"
  smallest_flavor    = "XS"
  biggest_flavor     = "XS"
  min_instance_count = 1
  max_instance_count = 1
  environment        = {
    CLEVER_COMPOSE_SECRET_API_TOKEN   = var.secret_api_token
    CLEVER_COMPOSE_SECRET_DB_PASSWORD = file("${path.module}/../../db_password.txt")
    DB_PASSWORD_FILE                  = "/run/secrets/db_password"
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
}
