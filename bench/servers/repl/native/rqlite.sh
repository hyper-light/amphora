#!/usr/bin/env bash
# rqlite, three voters over Raft, each rqlited a process on this host (setup.sh's release binary):
# HTTP on 127.0.0.1:54200-54202, Raft on 54210-54212. A write to a follower is forwarded to the
# leader; it's acknowledged once a quorum's Raft logs hold it, fsynced. Reads with level=none come
# from the node's own copy.
#
# Durability, from rqlite v10.7.4's source (what setup.sh fetches; re-check on a newer tag):
# - The Raft log is BoltDB through raft-boltdb (store/log/log.go:22-27), opened without NoSync, so
#   every log write's transaction ends in fdatasync (bbolt v1.5.0 tx.go:566-568; db.go:44-55 says
#   NoSync is for bulk loads only). NoFreelistSync is a store field no rqlited flag sets
#   (store/store.go:423 and 740; cmd/rqlited/flags.toml has no sync flag), and it skips only the
#   freelist's sync, not the commit's.
# - hashicorp/raft v1.8.0 counts the leader's own vote for an entry only after StoreLogs returns
#   (raft.go:1270-1278), and a follower answers AppendEntries only after its StoreLogs (raft.go:1562).
# - rqlited answers a write once raft.Apply's future resolves, after commit (store/store.go:1517-1518).
# - SQLite itself runs synchronous=OFF and is rebuilt from the log (rqlite.io/docs/design/, "SQLite").
# So rqlite's defaults already are "2 of 3 members' logs hold it, fsynced": there is no stronger
# setting and no weaker one is used. (Queued writes, `queue=1`, would answer before commit: the
# harness doesn't use them.)
set -euo pipefail
SYS=rqlite
PORT=${AMPHORA_BENCH_REPL_PORT:-54200}
DURABILITY="acknowledged once a quorum (2 of 3) of Raft logs hold it, each fdatasynced (BoltDB without NoSync); rqlited's defaults, which no flag weakens or strengthens"
# rqlited's own ready line (cmd/rqlited/main.go:256 at v10.7.4), printed after createCluster
# returns; with -bootstrap-expect that's once the node knows a leader (main.go:642-645, 672-676).
READY='node HTTP API available at '
. "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"

run_node() {
  local n=$1
  need_bin rqlited
  spawn "$n" "$BIN/rqlited" -node-id "$n" -http-addr "127.0.0.1:$(port "$n")" -raft-addr "127.0.0.1:$(port2 "$n")" \
    -bootstrap-expect 3 -join "127.0.0.1:$(port2 0),127.0.0.1:$(port2 1),127.0.0.1:$(port2 2)" "$(dir "$n")/data"
}

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

help_text() { "$BIN/rqlited" -h; }

main "$@"
