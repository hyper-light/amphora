#!/usr/bin/env bash
# CedarDB, Community Edition (its official image), for the bench's `cedardb` target (src/pg.rs), on
# 127.0.0.1:55434, Postgres's protocol.
#
# Durability: on a drive with a volatile write cache, as fedora's is (/sys/block/*/queue/write_cache
# says "write back"), CedarDB acknowledges a commit before it is flushed unless told otherwise
# (async_commit = default; its docs, "Write Caching"). The bench's connections each set
# async_commit = off, so a commit is acknowledged once flushed, as PostgreSQL's and Amphora's are.
# Memory as it ships: its buffer pool and working memory each take up to 45% of the memory it sees
# (here the container's 8 GiB cap), touched as used. The Community Edition holds 64 GiB at most.
#
# usage: servers/cedardb.sh start|stop|restart|reset|settings
#   reset drops the data directory and starts on a new one.
set -euo pipefail
NAME=amphora-bench-cedardb
IMAGE=${IMAGE:-cedardb/cedardb:v2026-10-08}
PORT=55434
DATA=${AMPHORA_BENCH_DATA:-$HOME/.cache/amphora-bench-data-nocow}/cedardb
# The image wants a password for its superuser; the server is reachable from this machine only.
PASSWORD=amphorabenchlocalonly2026
. "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"

# The image's entrypoint makes the database, sets its superuser's password and starts the server on
# every address (-address=::), which on the host's network would be every interface; the server takes
# an option once, so this takes the entrypoint's steps itself, with localhost's address.
SERVE='set -euo pipefail
db=/var/lib/cedardb/data/database
if [ -d "$db" ]; then
  catalogupgrade "$db"
else
  cedardb -interactive -createdb "$db" /dev/null <(echo "\q")
  cedardb -interactive "$db" /dev/null <(echo "ALTER USER postgres WITH PASSWORD '\''$PASSWORD'\'';")
fi
exec cedardb "$db" -address=127.0.0.1 -port="$PORT" -pgSocketDir=/var/run/postgresql'

run() {
  # Its socket directory belongs to the image's own user; the calling user gets one of its own. No
  # health check (the only image here with one): it would run a probe inside the server's
  # container through every run.
  docker run -d --name "$NAME" --network host --user "$(id -u):$(id -g)" --memory 8g --memory-swap 8g --tmpfs /var/run/postgresql:mode=1777 --no-healthcheck \
    -e HOME=/var/lib/cedardb/data -e PORT="$PORT" -e PASSWORD="$PASSWORD" -v "$DATA:/var/lib/cedardb/data" \
    --entrypoint bash "$IMAGE" -c "$SERVE"
}

# psql from the PostgreSQL image: CedarDB's has none.
sql() {
  docker run --rm --network host -e PGPASSWORD="$PASSWORD" postgres:17 psql -h 127.0.0.1 -p "$PORT" -U postgres -AtX -c "$1"
}

ready() {
  sql "SELECT 1"
}

settings() {
  local version commit
  version=$(sql "SELECT version()")
  commit=$(sql "SHOW async_commit")
  printf '{"version": "%s", "async_commit_server_default": "%s", "async_commit_bench_sessions": "off"}\n' "$version" "$commit"
}

main "$@"
