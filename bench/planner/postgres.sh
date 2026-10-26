#!/usr/bin/env bash
# PostgreSQL 17 for the planner bench, the official image as it ships but for its port: its data
# on a volume of Docker's own (inside Docker Desktop's VM on Windows and macOS, not a mount of the
# host's files through it), published on 127.0.0.1:55432, so it runs where Docker has no host
# network as where it has one. bench/servers/postgres.sh is the store benchmarks' (host network).
#
# usage: bench/planner/postgres.sh start|stop|settings
set -euo pipefail
# Git for Windows' shell rewrites arguments that look like paths (the volume's mount point).
export MSYS_NO_PATHCONV=1
NAME=amphora-planner-postgres
IMAGE=${IMAGE:-postgres:17}
case "${1:-}" in
  start)
    if [[ -z "$(docker ps -q -f "name=^${NAME}$")" ]]; then
      docker rm -f "$NAME" >/dev/null 2>&1 || true
      docker run -d --name "$NAME" -p 127.0.0.1:55432:5432 --shm-size 1g \
        -e POSTGRES_HOST_AUTH_METHOD=trust -v amphora-planner-postgres:/var/lib/postgresql/data "$IMAGE" >/dev/null
    fi
    # Ready when it answers, for at most two minutes.
    for _ in $(seq 1 120); do
      if docker exec "$NAME" pg_isready -q -U postgres; then
        exit 0
      fi
      sleep 1
    done
    echo "postgres did not answer in 120 s" >&2
    exit 1
    ;;
  stop) docker stop "$NAME" >/dev/null ;;
  settings) docker exec "$NAME" psql -U postgres -AtX -c "SELECT version()" ;;
  *)
    echo "usage: $0 start|stop|settings" >&2
    exit 2
    ;;
esac
