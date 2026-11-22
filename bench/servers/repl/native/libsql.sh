#!/usr/bin/env bash
# libSQL's server (sqld), one primary and two replicas, each a process on this host (setup.sh's
# release binary): HTTP (Hrana over HTTP, /v2/pipeline) on 127.0.0.1:54220-54222, the primary's
# gRPC for its replicas on 54230. Replicas forward writes to the primary and follow it; there is no
# election, so node 0 is always the leader.
set -euo pipefail
SYS=libsql
PORT=${AMPHORA_BENCH_REPL_PORT:-54220}
DURABILITY="acknowledged once the primary's SQLite WAL holds it (sqld's defaults); replicas follow asynchronously, so no quorum: one member's disk"
. "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"

run_node() {
  local n=$1
  need_bin sqld
  if [[ "$n" == 0 ]]; then
    spawn "$n" "$BIN/sqld" --db-path "$(dir "$n")/data" --http-listen-addr "127.0.0.1:$(port "$n")" --grpc-listen-addr "127.0.0.1:$(port2 0)"
  else
    spawn "$n" "$BIN/sqld" --db-path "$(dir "$n")/data" --http-listen-addr "127.0.0.1:$(port "$n")" --primary-grpc-url "http://127.0.0.1:$(port2 0)"
  fi
}

ready_node() { curl -sf "http://127.0.0.1:$(port "$1")/health" >/dev/null; }

leader() { ready_node 0 && echo 0; }

version() { "$BIN/sqld" --version; }

main "$@"
