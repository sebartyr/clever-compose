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
  name = "init-scripts-mongo-default"
  tags = ["compose-project=init-scripts-mongo", "compose-network=default", "compose-spec=384c4a34423c33de5c04e4446cf23a01"]
}

resource "clevercloud_mongodb" "mongo" {
  name          = "init-scripts-mongo-mongo"
  plan          = "xs_sml"
  region        = "par"
  networkgroups = [
    {
      networkgroup_id = clevercloud_networkgroup.default.id
      fqdn            = "mongo.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    },
  ]
}

resource "clevercloud_redis" "cache" {
  name          = "init-scripts-mongo-cache"
  plan          = "s_mono"
  region        = "par"
  networkgroups = [
    {
      networkgroup_id = clevercloud_networkgroup.default.id
      fqdn            = "cache.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    },
  ]
}

resource "clevercloud_docker" "mongo_init" {
  name               = "init-scripts-mongo-mongo-init"
  region             = "par"
  smallest_flavor    = "XS"
  biggest_flavor     = "XS"
  min_instance_count = 1
  max_instance_count = 1
  environment        = {
    CC_COMPOSE_INIT_HASH = "965bdbffd89e76fcb0544c283a25b57e"
  }
  deployment {
    repository = var.repository
    commit     = "refs/heads/clever-compose/mongo-init"
  }
  dependencies       = [clevercloud_mongodb.mongo.id]
}

resource "clevercloud_docker" "api" {
  name               = "init-scripts-mongo-api"
  region             = "par"
  smallest_flavor    = "XS"
  biggest_flavor     = "XS"
  min_instance_count = 1
  max_instance_count = 1
  environment        = {
    CC_COMPOSE_DNS_MAP = "mongo=${clevercloud_mongodb.mongo.id}.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    CC_COMPOSE_EXPAND  = "MONGO_URL"
    MONGO_URL          = "mongodb://$${MONGODB_ADDON_USER}:$${MONGODB_ADDON_PASSWORD}@${clevercloud_mongodb.mongo.id}.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud:8080/$${MONGODB_ADDON_DB}"
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
  depends_on         = [clevercloud_docker.mongo_init]
  dependencies       = [clevercloud_mongodb.mongo.id]
}
