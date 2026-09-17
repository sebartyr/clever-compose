//! Scripts d'initialisation et reprise de données (CLAUDE.md §6.3, phase
//! 5c) : les fichiers montés sur `/docker-entrypoint-initdb.d` d'un service
//! substitué s'exécutent contre l'addon par une Clever Task, une seule fois
//! par contenu grâce à une table témoin (`cc_compose_init`, hash des
//! scripts). Même mécanique pour `seed` avec un dump local. Formats par
//! fournisseur : `accepted_extensions` (initdb) et `seed_extensions`.

use std::path::Path as FsPath;

use cc_compose_model::plan::*;
use cc_compose_model::shim::{PlatformPort, ShimConfig};

use super::addons::Recognized;

/// Un script à rejouer, dans l'ordre alphabétique de l'image officielle.
#[derive(Debug, Clone)]
pub struct Script {
    pub name: String,
    /// Chemin sur le poste, relatif au projet.
    pub source: String,
}

/// Ce que l'image officielle du service source exécute depuis
/// `/docker-entrypoint-initdb.d` (et ce que la tâche sait rejouer). Redis
/// et Elasticsearch n'ont aucun mécanisme d'initialisation dans leur image :
/// des scripts montés là ne s'exécutent pas non plus en local.
pub fn accepted_extensions(provider: &str) -> &'static [&'static str] {
    match provider {
        "postgresql-addon" | "mysql-addon" => &[".sql", ".sql.gz", ".sh"],
        "mongodb-addon" | "documentdb" => &[".js", ".sh"],
        _ => &[],
    }
}

/// Ce que `seed` accepte comme dump, par fournisseur.
pub fn seed_extensions(provider: &str) -> &'static [&'static str] {
    match provider {
        // `.dump` : format custom de `pg_dump -Fc`, chargé par `pg_restore`.
        "postgresql-addon" => &[".sql", ".sql.gz", ".dump", ".sh"],
        "mysql-addon" => &[".sql", ".sql.gz", ".sh"],
        "mongodb-addon" | "documentdb" => &[".js", ".archive", ".sh"],
        "redis-addon" => &[".redis", ".resp", ".sh"],
        "es-addon" => &[".ndjson", ".sh"],
        _ => &[],
    }
}

fn has_ext(name: &str, exts: &[&str]) -> bool {
    exts.iter().any(|e| name.ends_with(e))
}

/// Scripts d'un service : fichiers montés (un fichier ou un répertoire)
/// vers `/docker-entrypoint-initdb.d`, triés par nom.
pub fn collect(service: &crate::compose::model::Service, rec: &Recognized, _project_dir: &FsPath, display: &dyn Fn(&str) -> String) -> Result<Vec<Script>, String> {
    let mut scripts: Vec<Script> = Vec::new();
    let exts = accepted_extensions(rec.provider);
    for v in &service.volumes {
        if v.kind != "bind" || !v.target.starts_with("/docker-entrypoint-initdb.d") {
            continue;
        }
        if exts.is_empty() {
            return Err(format!("`{}` monté vers `{}` : l'image {} n'exécute aucun script d'initialisation (pas de mécanisme dans l'image officielle) ; ce montage ne fait rien non plus en local. Retirez-le, ou chargez les données avec `clever-compose seed`", display(v.source.as_deref().unwrap_or("")), v.target, rec.kind));
        }
        let src = v.source.clone().unwrap_or_default();
        let meta = std::fs::metadata(&src).map_err(|_| format!("le chemin `{}` monté vers `{}` n'existe pas sur ce poste", display(&src), v.target))?;
        if meta.is_file() {
            let name = FsPath::new(&v.target).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| FsPath::new(&src).file_name().unwrap().to_string_lossy().to_string());
            scripts.push(Script { name, source: display(&src) });
        } else {
            let mut entries: Vec<_> = std::fs::read_dir(&src).map_err(|e| format!("{} : {e}", display(&src)))?.filter_map(|e| e.ok()).filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false)).collect();
            entries.sort_by_key(|e| e.file_name());
            for e in entries {
                let name = e.file_name().to_string_lossy().to_string();
                if !has_ext(&name, exts) {
                    continue;
                }
                scripts.push(Script { name, source: display(&e.path().to_string_lossy()) });
            }
        }
    }
    scripts.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(scripts)
}

/// Empreinte du contenu des scripts (ce qui décide de rejouer).
pub fn hash(scripts: &[Script], project_dir: &FsPath) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    for s in scripts {
        let p = project_dir.join(s.source.trim_start_matches("./"));
        h.update(s.name.as_bytes());
        h.update([0]);
        h.update(std::fs::read(&p).map_err(|e| format!("{} : {e}", p.display()))?);
        h.update([0]);
    }
    Ok(h.finalize().iter().take(16).map(|b| format!("{b:02x}")).collect())
}

/// Le client en ligne de commande dont l'exécution locale a besoin quand
/// Docker n'est pas là.
pub fn client(provider: &str) -> &'static str {
    match provider {
        "postgresql-addon" => "psql",
        "mysql-addon" => "mysql",
        "mongodb-addon" => "mongo",
        "documentdb" => "mongosh",
        "redis-addon" => "redis-cli",
        "es-addon" => "curl",
        _ => "sh",
    }
}

/// Image et script d'exécution selon le fournisseur.
pub fn runner(rec: &Recognized, version: Option<&str>) -> Option<(String, String)> {
    match rec.provider {
        "postgresql-addon" => Some((format!("postgres:{}", version.unwrap_or("17")), POSTGRES_RUNNER.to_string())),
        "mysql-addon" => Some((format!("mysql:{}", version.unwrap_or("8.4")), MYSQL_RUNNER.to_string())),
        // Le shell `mongo` de l'image de la version servie (4.0 : pas de
        // mongosh, et un mongosh récent ne parle plus à un 4.0).
        "mongodb-addon" => Some((format!("mongo:{}", version.map(|v| v.split('.').take(2).collect::<Vec<_>>().join(".")).unwrap_or_else(|| "4.0".into())), MONGO_RUNNER.to_string())),
        // DocumentDB parle MongoDB 7.0 : mongosh et les outils de l'image `mongo:7`.
        "documentdb" => Some(("mongo:7".to_string(), DOCUMENTDB_RUNNER.to_string())),
        // Pas d'image `redis` complète : redis-cli suffit, celle-ci est légère.
        "redis-addon" => Some((format!("redis:{}", version.unwrap_or("8")), REDIS_RUNNER.to_string())),
        // L'image elasticsearch pèse plus d'un gigaoctet pour un curl.
        "es-addon" => Some(("curlimages/curl:8.14.1".to_string(), ES_RUNNER.to_string())),
        _ => None,
    }
}

const POSTGRES_RUNNER: &str = r#"#!/bin/sh
# Généré par clever-compose : rejoue les scripts d'initialisation contre
# l'addon, une fois par contenu (table témoin cc_compose_init).
set -eu
export PGPASSWORD="$POSTGRESQL_ADDON_PASSWORD"
PSQL="psql -v ON_ERROR_STOP=1 -h $POSTGRESQL_ADDON_HOST -p $POSTGRESQL_ADDON_PORT -U $POSTGRESQL_ADDON_USER -d $POSTGRESQL_ADDON_DB"
$PSQL -qtAc "CREATE TABLE IF NOT EXISTS cc_compose_init (hash text PRIMARY KEY, applied_at timestamptz NOT NULL DEFAULT now())"
if [ "$($PSQL -qtAc "SELECT 1 FROM cc_compose_init WHERE hash = '$CLEVER_COMPOSE_INIT_HASH'")" = "1" ]; then
  echo "cc-compose init : scripts déjà appliqués ($CLEVER_COMPOSE_INIT_HASH)"
  exit 0
fi
# Variables des images officielles, pour les scripts .sh qui les lisent.
export POSTGRES_USER="$POSTGRESQL_ADDON_USER" POSTGRES_PASSWORD="$POSTGRESQL_ADDON_PASSWORD" POSTGRES_DB="$POSTGRESQL_ADDON_DB" PGHOST="$POSTGRESQL_ADDON_HOST" PGPORT="$POSTGRESQL_ADDON_PORT" PGUSER="$POSTGRESQL_ADDON_USER" PGDATABASE="$POSTGRESQL_ADDON_DB"
for f in "${CLEVER_COMPOSE_INIT_DIR:-/cc-init}"/*; do
  case "$f" in
    *.sh)     echo "cc-compose init : $f"; sh "$f" ;;
    *.sql)    echo "cc-compose init : $f"; $PSQL -f "$f" ;;
    *.sql.gz) echo "cc-compose init : $f"; gunzip -c "$f" | $PSQL ;;
    *.dump)   echo "cc-compose init : $f"; pg_restore --no-owner --no-privileges --no-acl -h "$POSTGRESQL_ADDON_HOST" -p "$POSTGRESQL_ADDON_PORT" -U "$POSTGRESQL_ADDON_USER" -d "$POSTGRESQL_ADDON_DB" "$f" ;;
    *)        echo "cc-compose init : $f ignoré" ;;
  esac
done
$PSQL -qtAc "INSERT INTO cc_compose_init (hash) VALUES ('$CLEVER_COMPOSE_INIT_HASH')"
echo "cc-compose init : terminé ($CLEVER_COMPOSE_INIT_HASH)"
"#;

const MYSQL_RUNNER: &str = r#"#!/bin/sh
# Généré par clever-compose : rejoue les scripts d'initialisation contre
# l'addon, une fois par contenu (table témoin cc_compose_init).
set -eu
export MYSQL_PWD="$MYSQL_ADDON_PASSWORD"
MYSQL="mysql -h $MYSQL_ADDON_HOST -P $MYSQL_ADDON_PORT -u $MYSQL_ADDON_USER $MYSQL_ADDON_DB"
$MYSQL -e "CREATE TABLE IF NOT EXISTS cc_compose_init (hash VARCHAR(64) PRIMARY KEY, applied_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP)"
if [ "$($MYSQL -N -s -e "SELECT 1 FROM cc_compose_init WHERE hash = '$CLEVER_COMPOSE_INIT_HASH'")" = "1" ]; then
  echo "cc-compose init : scripts déjà appliqués ($CLEVER_COMPOSE_INIT_HASH)"
  exit 0
fi
export MYSQL_USER="$MYSQL_ADDON_USER" MYSQL_PASSWORD="$MYSQL_ADDON_PASSWORD" MYSQL_DATABASE="$MYSQL_ADDON_DB" MYSQL_HOST="$MYSQL_ADDON_HOST"
for f in "${CLEVER_COMPOSE_INIT_DIR:-/cc-init}"/*; do
  case "$f" in
    *.sh)     echo "cc-compose init : $f"; sh "$f" ;;
    *.sql)    echo "cc-compose init : $f"; $MYSQL < "$f" ;;
    *.sql.gz) echo "cc-compose init : $f"; gunzip -c "$f" | $MYSQL ;;
    *)        echo "cc-compose init : $f ignoré" ;;
  esac
done
$MYSQL -e "INSERT INTO cc_compose_init (hash) VALUES ('$CLEVER_COMPOSE_INIT_HASH')"
echo "cc-compose init : terminé ($CLEVER_COMPOSE_INIT_HASH)"
"#;

const MONGO_RUNNER: &str = r#"#!/bin/sh
# Généré par clever-compose : rejoue les scripts d'initialisation contre
# l'addon, une fois par contenu (collection témoin cc_compose_init).
set -eu
MONGO="mongo --quiet $MONGODB_ADDON_URI"
if [ "$($MONGO --eval "print(db.cc_compose_init.findOne({_id: '$CLEVER_COMPOSE_INIT_HASH'}) ? 1 : 0)")" = "1" ]; then
  echo "cc-compose init : scripts déjà appliqués ($CLEVER_COMPOSE_INIT_HASH)"
  exit 0
fi
# Variables de l'image officielle, pour les scripts .sh qui les lisent.
export MONGO_INITDB_DATABASE="$MONGODB_ADDON_DB" MONGO_INITDB_ROOT_USERNAME="$MONGODB_ADDON_USER" MONGO_INITDB_ROOT_PASSWORD="$MONGODB_ADDON_PASSWORD"
for f in "${CLEVER_COMPOSE_INIT_DIR:-/cc-init}"/*; do
  case "$f" in
    *.sh)      echo "cc-compose init : $f"; sh "$f" ;;
    *.js)      echo "cc-compose init : $f"; $MONGO "$f" ;;
    *.archive) echo "cc-compose init : $f"; mongorestore --quiet --uri="$MONGODB_ADDON_URI" --archive="$f" --nsFrom='*.*' --nsTo="$MONGODB_ADDON_DB.*" ;;
    *)         echo "cc-compose init : $f ignoré" ;;
  esac
done
$MONGO --eval "db.cc_compose_init.insertOne({_id: '$CLEVER_COMPOSE_INIT_HASH', applied_at: new Date()})"
echo "cc-compose init : terminé ($CLEVER_COMPOSE_INIT_HASH)"
"#;

const DOCUMENTDB_RUNNER: &str = r#"#!/bin/sh
# Généré par clever-compose : rejoue les scripts d'initialisation contre
# l'endpoint DocumentDB de l'addon, une fois par contenu (collection témoin
# cc_compose_init). TLS imposé, certificat hors des autorités publiques.
set -eu
URI="$DOCUMENTDB_ADDON_URI"
MONGO="mongosh --quiet --tls --tlsAllowInvalidCertificates $URI"
if [ "$($MONGO --eval "print(db.cc_compose_init.findOne({_id: '$CLEVER_COMPOSE_INIT_HASH'}) ? 1 : 0)")" = "1" ]; then
  echo "cc-compose init : scripts déjà appliqués ($CLEVER_COMPOSE_INIT_HASH)"
  exit 0
fi
# Variables de l'image officielle mongo, pour les scripts .sh qui les lisent.
export MONGO_INITDB_DATABASE="$POSTGRESQL_ADDON_DB" MONGO_INITDB_ROOT_USERNAME="$POSTGRESQL_ADDON_USER" MONGO_INITDB_ROOT_PASSWORD="$POSTGRESQL_ADDON_PASSWORD"
for f in "${CLEVER_COMPOSE_INIT_DIR:-/cc-init}"/*; do
  case "$f" in
    *.sh)      echo "cc-compose init : $f"; sh "$f" ;;
    *.js)      echo "cc-compose init : $f"; $MONGO "$f" ;;
    *.archive) echo "cc-compose init : $f"; mongorestore --quiet --ssl --sslAllowInvalidCertificates --uri="$URI" --archive="$f" --nsFrom='*.*' --nsTo="$POSTGRESQL_ADDON_DB.*" ;;
    *)         echo "cc-compose init : $f ignoré" ;;
  esac
done
$MONGO --eval "db.cc_compose_init.insertOne({_id: '$CLEVER_COMPOSE_INIT_HASH', applied_at: new Date()})"
echo "cc-compose init : terminé ($CLEVER_COMPOSE_INIT_HASH)"
"#;

const REDIS_RUNNER: &str = r#"#!/bin/sh
# Généré par clever-compose : charge les données dans l'addon, une fois par
# contenu (clé témoin cc_compose_init:<hash>).
set -eu
export REDISCLI_AUTH="$REDIS_PASSWORD"
REDIS="redis-cli -h $REDIS_HOST -p $REDIS_PORT"
if [ "$($REDIS EXISTS "cc_compose_init:$CLEVER_COMPOSE_INIT_HASH")" = "1" ]; then
  echo "cc-compose init : données déjà chargées ($CLEVER_COMPOSE_INIT_HASH)"
  exit 0
fi
for f in "${CLEVER_COMPOSE_INIT_DIR:-/cc-init}"/*; do
  case "$f" in
    *.sh)    echo "cc-compose init : $f"; sh "$f" ;;
    *.redis) echo "cc-compose init : $f"; $REDIS < "$f" ;;
    *.resp)  echo "cc-compose init : $f"; $REDIS --pipe < "$f" ;;
    *)       echo "cc-compose init : $f ignoré" ;;
  esac
done
$REDIS SET "cc_compose_init:$CLEVER_COMPOSE_INIT_HASH" "$(date -u +%Y-%m-%dT%H:%M:%SZ)" > /dev/null
echo "cc-compose init : terminé ($CLEVER_COMPOSE_INIT_HASH)"
"#;

const ES_RUNNER: &str = r#"#!/bin/sh
# Généré par clever-compose : charge les données dans l'addon, une fois par
# contenu (index témoin cc_compose_init, document <hash>).
set -eu
ES="https://$ES_ADDON_HOST"
export ELASTICSEARCH_URL="$ES" ELASTICSEARCH_USERNAME="$ES_ADDON_USER" ELASTICSEARCH_PASSWORD="$ES_ADDON_PASSWORD"
# Identifiants hors ligne de commande (visibles dans `ps` sinon).
printf 'machine %s login %s password %s\n' "$ES_ADDON_HOST" "$ES_ADDON_USER" "$ES_ADDON_PASSWORD" > "${TMPDIR:-/tmp}/cc-init.netrc"
chmod 600 "${TMPDIR:-/tmp}/cc-init.netrc"
# es <méthode> <chemin> [options curl] : corps dans cc-init-es.out, code HTTP
# dans $CODE (pas de substitution de commande : elle perdrait la variable).
es() { m="$1"; p="$2"; shift 2; CODE="$(curl -sS --netrc-file "${TMPDIR:-/tmp}/cc-init.netrc" -o "${TMPDIR:-/tmp}/cc-init-es.out" -w '%{http_code}' -X "$m" "$ES$p" "$@")"; }
es GET "/cc_compose_init/_doc/$CLEVER_COMPOSE_INIT_HASH"
if [ "$CODE" = "200" ]; then
  echo "cc-compose init : données déjà chargées ($CLEVER_COMPOSE_INIT_HASH)"
  exit 0
fi
for f in "${CLEVER_COMPOSE_INIT_DIR:-/cc-init}"/*; do
  case "$f" in
    *.sh)     echo "cc-compose init : $f"; sh "$f" ;;
    *.ndjson) echo "cc-compose init : $f"
              es POST "/_bulk?refresh=true" -H 'Content-Type: application/x-ndjson' --data-binary "@$f"
              case "$CODE:$(cat "${TMPDIR:-/tmp}/cc-init-es.out")" in 2*:*'"errors":false'*) ;; *) head -c 2000 "${TMPDIR:-/tmp}/cc-init-es.out"; echo; echo "cc-compose init : échec du bulk $f (HTTP $CODE)"; exit 1 ;; esac ;;
    *)        echo "cc-compose init : $f ignoré" ;;
  esac
done
es PUT "/cc_compose_init/_doc/$CLEVER_COMPOSE_INIT_HASH" -H 'Content-Type: application/json' -d "{\"applied_at\":\"$(date -u +%Y-%m-%dT%H:%M:%SZ)\"}"
case "$CODE" in 2*) ;; *) echo "cc-compose init : témoin non écrit (HTTP $CODE)"; exit 1 ;; esac
echo "cc-compose init : terminé ($CLEVER_COMPOSE_INIT_HASH)"
"#;

#[allow(clippy::too_many_arguments)]
/// La tâche d'initialisation d'un addon.
pub fn task(project: &str, service: &str, rec: &Recognized, version: Option<&str>, scripts: &[Script], hash: &str, region: &str, tags: Vec<String>, intrinsics: ImageIntrinsics, image: String, script: String) -> App {
    let _ = rec;
    let name = format!("{service}-init");
    let files: Vec<CopiedFile> = scripts.iter().map(|s| CopiedFile { source: s.source.clone(), target: format!("/cc-init/{}", s.name), read_only: true }).collect();
    let _ = version;
    App {
        id: ResourceId(format!("app/{name}")),
        service: name.clone(),
        name: format!("{project}-{name}"),
        kind: AppKind::Task,
        region: region.to_string(),
        scaling: Scaling { min_flavor: super::flavors::docker().default_flavor.clone(), max_flavor: super::flavors::docker().default_flavor.clone(), min_instances: 1, max_instances: 1 },
        source: Source::Synthetic { image, private_registry: false, files, generated: vec![GeneratedFile { target: "/cc-init.sh".into(), content: script, executable: true }] },
        image: intrinsics,
        shim: ShimConfig { argv: vec!["sh".into(), "/cc-init.sh".into()], workdir: None, user: None, port: PlatformPort::Health { ready_port: None }, tcp: None, udp: None, secrets: Vec::new() },
        env: vec![EnvVar { key: "CLEVER_COMPOSE_INIT_HASH".into(), value: Template::literal(hash) }],
        depends_on: vec![ResourceId(format!("addon/{service}"))],
        health_check_path: None,
        secrets: Vec::new(),
        tags,
        run: RunWhere::Platform,
    }
}

/// `clever-compose seed` : un plan réduit à l'addon du service et à une
/// tâche `<service>-seed` qui charge `dump` (extensions par fournisseur,
/// `seed_extensions`), avec le même témoin (un dump inchangé n'est pas
/// rechargé). Pour Redis et Elasticsearch, dont l'image n'a pas de scripts
/// d'initialisation, c'est le seul chemin de reprise de données.
pub fn seed_plan(plan: &Plan, service: &str, dump: &FsPath, project_dir: &FsPath, images: &dyn super::images::ImageResolver, local: bool) -> Result<Plan, String> {
    let addon = plan
        .resources
        .iter()
        .find_map(|r| match r {
            Resource::Addon(a) if a.service == service => Some(a.clone()),
            _ => None,
        })
        .ok_or_else(|| format!("`{service}` n'est pas un service substitué par un addon dans ce plan"))?;
    let mut rec = super::addons::recognize(match addon.provider.as_str() {
        "postgresql-addon" if addon.options.get("documentdb").is_some_and(|v| v == "true") => "mongo",
        "postgresql-addon" => "postgres",
        "mysql-addon" => "mysql",
        "mongodb-addon" => "mongo",
        "redis-addon" => "redis",
        "es-addon" => "elasticsearch",
        other => return Err(format!("seed non supporté pour {other}")),
    })
    .ok_or("fournisseur non reconnu")?;
    if addon.options.get("documentdb").is_some_and(|v| v == "true") {
        rec.provider = super::addons::DOCUMENTDB;
    }
    let abs = std::fs::canonicalize(dump).map_err(|e| format!("{} : {e}", dump.display()))?;
    let rel = match abs.strip_prefix(project_dir) {
        Ok(r) => format!("./{}", r.display()),
        Err(_) => return Err(format!("le dump {} doit être dans le répertoire du projet {}", abs.display(), project_dir.display())),
    };
    let name = abs.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "dump.sql".into());
    let exts = seed_extensions(rec.provider);
    if !has_ext(&name, exts) {
        return Err(format!("`{name}` : pour {} un dump est un fichier {}", rec.kind, exts.join(", ")));
    }
    let scripts = vec![Script { name, source: rel }];
    let h = hash(&scripts, project_dir)?;
    let (image, script) = runner(&rec, addon.version.as_deref()).ok_or("fournisseur sans image d'exécution")?;
    let intr = images.resolve(&image)?;
    let tags: Vec<String> = addon.tags.iter().map(|t| if t.starts_with("compose-service=") { format!("compose-service={service}-seed") } else { t.clone() }).collect();
    let mut task = task(&plan.project, service, &rec, addon.version.as_deref(), &scripts, &h, &addon.region, tags, intr, image, script);
    task.service = format!("{service}-seed");
    task.name = format!("{}-{service}-seed", plan.project);
    task.id = ResourceId(format!("app/{service}-seed"));
    if local {
        task.run = RunWhere::Local;
    }
    Ok(Plan { project: plan.project.clone(), spec_hash: plan.spec_hash.clone(), resources: vec![Resource::Addon(addon), Resource::App(task)], assets: false })
}
