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
  name = "wordpress-addon-default"
  tags = ["compose-project=wordpress-addon", "compose-network=default", "compose-spec=a964eb627f2fac67e8cbb8509eb6ad4c"]
}

resource "clevercloud_mysql" "db" {
  name    = "wordpress-addon-db"
  plan    = "dev"
  region  = "par"
  version = "8.4"
}

resource "clevercloud_docker" "wordpress" {
  name               = "wordpress-addon-wordpress"
  region             = "par"
  smallest_flavor    = "XS"
  biggest_flavor     = "XS"
  min_instance_count = 1
  max_instance_count = 1
  environment        = {
    CC_COMPOSE_EXPAND     = "WORDPRESS_DB_HOST,WORDPRESS_DB_USER,WORDPRESS_DB_PASSWORD,WORDPRESS_DB_NAME"
    WORDPRESS_DB_HOST     = "$${MYSQL_ADDON_HOST}"
    WORDPRESS_DB_NAME     = "$${MYSQL_ADDON_DB}"
    WORDPRESS_DB_PASSWORD = "$${MYSQL_ADDON_PASSWORD}"
    WORDPRESS_DB_USER     = "$${MYSQL_ADDON_USER}"
  }
  networkgroups      = [
    {
      networkgroup_id = clevercloud_networkgroup.default.id
      fqdn            = "wordpress.m.${clevercloud_networkgroup.default.id}.cc-ng.cloud"
    },
  ]
  deployment {
    repository = var.repository
    commit     = "refs/heads/clever-compose/wordpress"
  }
  dependencies       = [clevercloud_mysql.db.id]
}
