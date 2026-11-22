#!/usr/bin/env bash
# dqlite, three voters over Raft, through go-dqlite's app package, for bench/src/repl.rs's `dqlite`
# system: the bench's shim (shim/main.go, built here into amphora-bench-repl-shim) takes /exec and
# /count on 127.0.0.1:54120-54122. go-dqlite sends every statement to the leader; a commit is
# acknowledged once a quorum's Raft logs hold it.
set -euo pipefail
SYS=dqlite
IMAGE=${IMAGE:-amphora-bench-repl-shim:local}
PORT=${AMPHORA_BENCH_REPL_PORT:-54120}
HERE="$(dirname -- "${BASH_SOURCE[0]}")"
. "$HERE/lib.sh"

run_node() {
  local n=$1 join=()
  [[ "$n" == 0 ]] || join=(-join "$(node 0):9001")
  docker image inspect "$IMAGE" >/dev/null 2>&1 || docker build -q -t "$IMAGE" "$HERE/shim" >/dev/null
  docker run -d --name "$(node "$n")" --network "$NET" -p "127.0.0.1:$(port "$n"):8080" "$IMAGE" \
    shim -backend dqlite -dir /data -address "$(node "$n"):9001" "${join[@]}" -listen :8080
}

ready_node() { curl -sf "http://127.0.0.1:$(port "$1")/ready" >/dev/null; }

# The leader's address as any live node's shim says it, as its node number.
leader() {
  local n addr
  for n in 0 1 2; do
    addr=$(curl -sf --max-time 2 "http://127.0.0.1:$(port "$n")/leader" 2>/dev/null || true)
    if [[ "$addr" =~ -([0-9]):9001 ]]; then
      echo "${BASH_REMATCH[1]}"
      return 0
    fi
  done
}

main "$@"
