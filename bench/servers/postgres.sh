#!/usr/bin/env bash
# PostgreSQL 17, the official image, for the bench's `postgres` target (src/pg.rs), on
# 127.0.0.1:55432.
#
# Durable as it ships: fsync = on, synchronous_commit = on, full_page_writes = on, so a commit is
# acknowledged once its log is flushed, as Amphora's is. Memory as it ships too (shared_buffers
# 128MB), so its memory compares with Amphora's on equal terms. The one change: max_connections =
# 1100, room for --connections up to 1024 with the readers and the search beside them.
#
# usage: servers/postgres.sh start|stop|restart|reset|settings
#   reset drops the data directory and starts on a new one.
set -euo pipefail
NAME=amphora-bench-postgres
IMAGE=${IMAGE:-postgres:17}
PORT=55432
DATA=${AMPHORA_BENCH_DATA:-$HOME/.cache/amphora-bench-data-nocow}/postgres
. "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"

run() {
  # Parallel queries take their shared memory from /dev/shm: Docker's 64 MB can run out.
  docker run -d --name "$NAME" --network host --user "$(id -u):$(id -g)" --memory 8g --memory-swap 8g --shm-size 256m \
    -e POSTGRES_HOST_AUTH_METHOD=trust -v "$DATA:/var/lib/postgresql/data" \
    "$IMAGE" postgres -c listen_addresses=127.0.0.1 -c port="$PORT" -c max_connections=1100
}

ready() {
  docker exec "$NAME" pg_isready -q -h 127.0.0.1 -p "$PORT" -U postgres
}

settings() {
  docker exec "$NAME" psql -h 127.0.0.1 -p "$PORT" -U postgres -AtX -c "SELECT jsonb_build_object('version', version()) || jsonb_object_agg(n, current_setting(n)) FROM unnest(ARRAY['fsync', 'synchronous_commit', 'full_page_writes', 'wal_sync_method', 'wal_level', 'commit_delay', 'shared_buffers', 'max_connections', 'max_wal_size', 'checkpoint_timeout']) AS n"
}

main "$@"
