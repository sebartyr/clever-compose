# Dépendances vendorisées

## saphyr-parser 0.0.12

Copie de la crate publiée, avec un seul changement dans `src/scanner.rs`
(`fetch_next_token`) : la vérification d'indentation est désactivée en
contexte flow (`flow_level > 0`), comme le fait libyaml, dont go-yaml est le
port et que Compose utilise. Sans cela, le `docker-compose.yml` de Sentry
(crochet fermant aligné sur sa clé) est refusé alors que `docker compose`
l'accepte.

Chercher `clever-compose :` dans le fichier pour retrouver le patch. À
proposer en amont sous forme d'option de tolérance ; à supprimer si elle
est acceptée.
