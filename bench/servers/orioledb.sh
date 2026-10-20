#!/usr/bin/env bash
# OrioleDB (PostgreSQL 17 with OrioleDB's table access method; the project's image, release beta19)
# for the bench's `orioledb` target (src/pg.rs: every table `USING orioledb`), on 127.0.0.1:55433.
#
# Durable as PostgreSQL ships: fsync = on, synchronous_commit = on, full_page_writes = on. OrioleDB
# writes its row-level log into PostgreSQL's, so a commit is acknowledged once that log is flushed.
# Memory as its image ships it: shared_buffers 128MB beside orioledb.main_buffers 512MB and
# orioledb.undo_buffers 256MB (OrioleDB's own defaults are 64MB and 1MB; the image raises them),
# and max_wal_size 8GB; so its memory doesn't compare with PostgreSQL's 128MB. Those settings live in
# the image's /etc/postgresql, which only its postgres user may read, so they're given here as flags
# instead, the same values. The one change: max_connections = 1100, as for PostgreSQL.
#
# usage: servers/orioledb.sh start|stop|restart|reset|settings
#   reset drops the data directory and starts on a new one.
set -euo pipefail
NAME=amphora-bench-orioledb
IMAGE=${IMAGE:-orioledb/orioledb:beta19-pg17}
PORT=55433
DATA=${AMPHORA_BENCH_DATA:-$HOME/.cache/amphora-bench-data-nocow}/orioledb
. "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"

run() {
  # initdb wants the calling user in /etc/passwd, and this image has no nss_wrapper to fake one.
  docker run -d --name "$NAME" --network host --user "$(id -u):$(id -g)" -v /etc/passwd:/etc/passwd:ro --memory 8g --memory-swap 8g --shm-size 256m \
    -e POSTGRES_HOST_AUTH_METHOD=trust -v "$DATA:/var/lib/postgresql/data" \
    "$IMAGE" postgres -c listen_addresses=127.0.0.1 -c port="$PORT" -c max_connections=1100 \
    -c shared_preload_libraries=orioledb -c default_table_access_method=orioledb -c orioledb.main_buffers=512MB -c orioledb.undo_buffers=256MB -c max_wal_size=8GB
}

ready() {
  docker exec "$NAME" pg_isready -q -h 127.0.0.1 -p "$PORT" -U postgres
}

settings() {
  docker exec "$NAME" psql -h 127.0.0.1 -p "$PORT" -U postgres -AtX -c "SELECT jsonb_build_object('version', version()) || jsonb_object_agg(n, current_setting(n)) FROM unnest(ARRAY['fsync', 'synchronous_commit', 'full_page_writes', 'wal_sync_method', 'wal_level', 'commit_delay', 'shared_buffers', 'orioledb.main_buffers', 'orioledb.undo_buffers', 'shared_preload_libraries', 'default_table_access_method', 'max_connections', 'max_wal_size', 'checkpoint_timeout']) AS n"
}

main "$@"
