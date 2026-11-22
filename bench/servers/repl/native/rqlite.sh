#!/usr/bin/env bash
# rqlite, three voters over Raft, each rqlited a process on this host (setup.sh's release binary):
# HTTP on 127.0.0.1:54200-54202, Raft on 54210-54212. A write to a follower is forwarded to the
# leader; it's acknowledged once a quorum's Raft logs hold it, each written to disk before it says so
# (rqlite's one write durability). Reads with level=none come from the node's own copy.
set -euo pipefail
SYS=rqlite
PORT=${AMPHORA_BENCH_REPL_PORT:-54200}
DURABILITY="acknowledged once a quorum (2 of 3) of Raft logs hold it on disk; rqlited's defaults, no flags changing durability"
. "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"

run_node() {
  local n=$1
  need_bin rqlited
  spawn "$n" "$BIN/rqlited" -node-id "$n" -http-addr "127.0.0.1:$(port "$n")" -raft-addr "127.0.0.1:$(port2 "$n")" \
    -bootstrap-expect 3 -join "127.0.0.1:$(port2 0),127.0.0.1:$(port2 1),127.0.0.1:$(port2 2)" "$(dir "$n")/data"
}

ready_node() { curl -sf "http://127.0.0.1:$(port "$1")/readyz" >/dev/null; }

leader() {
  local n id
  for n in 0 1 2; do
    id=$(curl -sf --max-time 2 "http://127.0.0.1:$(port "$n")/status" 2>/dev/null | tr -d ' \n' | grep -o '"leader":{[^}]*}' | grep -o '"node_id":"[0-9]*"' | grep -o '[0-9]\+' || true)
    if [[ -n "$id" ]]; then
      echo "$id"
      return 0
    fi
  done
}

version() { "$BIN/rqlited" -version; }

main "$@"
