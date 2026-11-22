#!/usr/bin/env bash
# dqlite, three voters over Raft, through go-dqlite's app package: the bench's shim (../shim/main.go,
# built by setup.sh against the host's libdqlite) a process on this host for each node, /exec and
# /count on 127.0.0.1:54240-54242, dqlite's own protocol on 54250-54252. go-dqlite sends every
# statement to the leader; a commit is acknowledged once a quorum's Raft logs hold it.
set -euo pipefail
SYS=dqlite
PORT=${AMPHORA_BENCH_REPL_PORT:-54240}
DURABILITY="acknowledged once a quorum (2 of 3) of Raft logs hold it; libdqlite's and go-dqlite app's defaults"
. "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"

run_node() {
  local n=$1 join=()
  need_bin shim
  [[ "$n" == 0 ]] || join=(-join "127.0.0.1:$(port2 0)")
  mkdir -p "$(dir "$n")/data"
  spawn "$n" env LD_LIBRARY_PATH="$BIN/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" "$BIN/shim" -backend dqlite -dir "$(dir "$n")/data" \
    -address "127.0.0.1:$(port2 "$n")" "${join[@]}" -listen "127.0.0.1:$(port "$n")"
}

ready_node() { curl -sf "http://127.0.0.1:$(port "$1")/ready" >/dev/null; }

leader() {
  local n addr
  for n in 0 1 2; do
    addr=$(curl -sf --max-time 2 "http://127.0.0.1:$(port "$n")/leader" 2>/dev/null || true)
    if [[ "$addr" =~ :([0-9]+)$ ]]; then
      echo $((BASH_REMATCH[1] - PORT - 10))
      return 0
    fi
  done
}

version() { cat "$BIN/dqlite.version"; }

help_text() { :; }

main "$@"
