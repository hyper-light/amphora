#!/usr/bin/env bash
# CockroachDB, one node a region (--locality=region=r<n>), insecure, for bench/src/scale.rs's
# `cockroach` system: SQL on 127.0.0.1:PORT+n. A commit is acknowledged once a quorum of its range's
# replicas holds it (Raft), with Parallel Commits across ranges.
set -euo pipefail
SYS=cockroach
IMAGE=${IMAGE:-cockroachdb/cockroach:latest}
IMAGES=$IMAGE
PORT=${AMPHORA_BENCH_SCALE_PORT:-56200}
. "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"

containers() { name "$1"; }

run_node() {
  local n=$1 join="" m
  for ((m = 0; m < $(nodes); m++)); do join+="${join:+,}$(name "$m"):26257"; done
  docker run -d --name "$(name "$n")" --hostname "$(name "$n")" --label "$LABEL" --network "$NET" -p "127.0.0.1:$(port "$n"):26257" "$IMAGE" \
    start --insecure --join="$join" --advertise-addr="$(name "$n"):26257" --listen-addr=0.0.0.0:26257 --http-addr=0.0.0.0:8080 --locality="region=r$n"
}

after_up() {
  wait_for docker exec "$(name 0)" ./cockroach init --insecure --host="$(name 0):26257" || docker exec "$(name 0)" ./cockroach node status --insecure --host="$(name 0):26257" >/dev/null
}

ready_node() { docker exec "$(name "$1")" ./cockroach sql --insecure --host="$(name "$1"):26257" -e "SELECT 1" >/dev/null; }

endpoint() { echo "host=127.0.0.1 port=$(port "$1") user=root dbname=defaultdb sslmode=disable"; }

main "$@"
