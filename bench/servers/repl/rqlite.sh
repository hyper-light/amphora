#!/usr/bin/env bash
# rqlite, three voters over Raft, for bench/src/repl.rs's `rqlite` system: HTTP on 127.0.0.1:54100-54102.
# A write to a follower is forwarded to the leader; a commit is acknowledged once a quorum's Raft
# logs hold it (rqlite's only write durability). Reads with level=none come from the node's own copy.
set -euo pipefail
SYS=rqlite
IMAGE=${IMAGE:-rqlite/rqlite:latest}
PORT=${AMPHORA_BENCH_REPL_PORT:-54100}
. "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"

run_node() {
  local n=$1
  docker run -d --name "$(node "$n")" --network "$NET" -p "127.0.0.1:$(port "$n"):4001" --entrypoint rqlited "$IMAGE" \
    -node-id "$n" -http-addr 0.0.0.0:4001 -http-adv-addr "$(node "$n"):4001" -raft-addr 0.0.0.0:4002 -raft-adv-addr "$(node "$n"):4002" \
    -bootstrap-expect 3 -join "$(node 0):4002,$(node 1):4002,$(node 2):4002" /rqlite/file/data
}

ready_node() { curl -sf "http://127.0.0.1:$(port "$1")/readyz" >/dev/null; }

# The leader's node ID as any live node's /status says it.
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

main "$@"
