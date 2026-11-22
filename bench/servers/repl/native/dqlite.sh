#!/usr/bin/env bash
# dqlite, three voters over Raft, through go-dqlite's app package: the bench's shim (../shim/main.go,
# built by setup.sh against the host's libdqlite) a process on this host for each node, /exec and
# /count on 127.0.0.1:54240-54242, dqlite's own protocol on 54250-54252. go-dqlite sends every
# statement to the leader; a commit is acknowledged once a quorum's Raft logs hold it.
#
# Durability, from libdqlite v1.18.7's source (setup.sh's build when the host has none):
# - Raft log segments are opened O_WRONLY|O_DSYNC (src/raft/uv_fs.c:311, UvFsAllocateFile, which
#   src/raft/uv_prepare.c:54 uses for every open segment), so a write returns once it's on disk.
# - The leader counts its own copy only from last_stored, set when its append completes
#   (src/raft/replication.c:471 appendLeaderCb, 562-566 replicationQuorum), and a follower reports
#   last_stored as its log's end (replication.c:1021).
# - dqlite keeps SQLite's pages in memory and its WAL frames in that log (include/dqlite.h:262-264).
# - Neither libdqlite's node API (include/dqlite.h: no sync setting) nor go-dqlite's app options
#   (app/options.go: none about syncing; WithDiskMode is experimental and moves the database to disk,
#   not the log) offer anything weaker or stronger.
# So the defaults already are "2 of 3 members' logs hold it, fsynced".
set -euo pipefail
SYS=dqlite
PORT=${AMPHORA_BENCH_REPL_PORT:-54240}
DURABILITY="acknowledged once a quorum (2 of 3) of Raft logs hold it, each written O_DSYNC; libdqlite's and go-dqlite app's defaults, which no setting changes"
# The shim's own ready line (../shim/main.go), printed once go-dqlite's App.Ready returned (the node
# joined, or bootstrapped as node 0) and its database is open, with its HTTP listener bound.
READY='shim ready: listening on '
. "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"

run_node() {
  local n=$1 join=()
  need_bin shim
  [[ "$n" == 0 ]] || join=(-join "127.0.0.1:$(port2 0)")
  mkdir -p "$(dir "$n")/data"
  spawn "$n" env LD_LIBRARY_PATH="$BIN/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" "$BIN/shim" -backend dqlite -dir "$(dir "$n")/data" \
    -address "127.0.0.1:$(port2 "$n")" "${join[@]}" -listen "127.0.0.1:$(port "$n")"
}

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

# The leader's configuration as the shim reads it (its /voters): go-dqlite's app can call a node
# ready while it's still a spare (../shim/main.go), so `up` refuses unless all three vote, and
# each record carries what the cluster said.
voters() {
  curl -sf --max-time "$BRINGUP_S" "http://127.0.0.1:$(port 0)/voters" | python3 -I -c 'import json, sys; c = json.load(sys.stdin); print(c["voters"], c["members"])'
}

after_up() {
  local v m
  read -r v m < <(voters) || { echo "dqlite: the shim didn't give the cluster's voters" >&2; return 1; }
  [[ "$v" == 3 && "$m" == 3 ]] || { echo "dqlite: $v of $m members vote once every node is ready, not 3 of 3: a commit's quorum isn't 2 of 3" >&2; return 1; }
}

durability_check() { voters | { read -r v m && echo "voters=$v of members=$m"; } || echo "voters unread"; }

version() { cat "$BIN/dqlite.version"; }

help_text() { :; }

main "$@"
