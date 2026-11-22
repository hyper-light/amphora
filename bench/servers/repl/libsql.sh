#!/usr/bin/env bash
# libSQL's server (sqld): one primary and two replicas, for bench/src/repl.rs's `libsql` system: HTTP
# (Hrana over HTTP, /v2/pipeline) on 127.0.0.1:54110-54112. Replicas forward writes to the primary
# and serve reads from their own copy; there is no election, so node 0 is always the leader.
set -euo pipefail
SYS=libsql
IMAGE=${IMAGE:-ghcr.io/tursodatabase/libsql-server:latest}
PORT=${AMPHORA_BENCH_REPL_PORT:-54110}
. "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"

run_node() {
  local n=$1
  if [[ "$n" == 0 ]]; then
    docker run -d --name "$(node "$n")" --network "$NET" -p "127.0.0.1:$(port "$n"):8080" -e SQLD_NODE=primary "$IMAGE"
  else
    docker run -d --name "$(node "$n")" --network "$NET" -p "127.0.0.1:$(port "$n"):8080" -e SQLD_NODE=replica -e SQLD_PRIMARY_URL="http://$(node 0):5001" "$IMAGE"
  fi
}

ready_node() { curl -sf "http://127.0.0.1:$(port "$1")/health" >/dev/null; }

leader() { ready_node 0 && echo 0; }

main "$@"
