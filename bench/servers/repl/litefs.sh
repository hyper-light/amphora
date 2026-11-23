#!/usr/bin/env bash
# LiteFS, three candidates for one Consul lease, each `litefs mount` a process on this host
# (setup.sh's release binaries), SQLite on each node's FUSE mount through the bench's shim
# (shim/main.go, which LiteFS runs once the node is primary or follows one) on
# 127.0.0.1:54260-54262; LiteFS's own HTTP on 54270-54272. The lease's holder is the primary; a
# commit is acknowledged once the primary's SQLite holds it (synchronous=FULL), and replicas stream it
# after (LiteFS replicates asynchronously). A write to a replica fails read-only, so the harness sends
# it on to the next node. Consul runs in dev mode beside the nodes (HTTP on 54280, its other ports
# 54281-54283, DNS off), and goes with them. FUSE needs /dev/fuse and fusermount3 on the host.
#
# Durability, from LiteFS v0.5.14's source (what setup.sh fetches) and its docs:
# - The shim opens SQLite with synchronous=FULL (shim/main.go), and LiteFS's CommitWAL, run as
#   SQLite releases its write lock inside COMMIT (db.go:1548-1553), fsyncs the WAL (db.go:1568-1569)
#   and the transaction's LTX file (db.go:1696) on the primary.
# - Replication is asynchronous: LiteFS "does not confirm that writes are committed to other nodes
#   before continuing to the next transaction", and "Synchronous replication & time-bound
#   asynchronous replication are features that are planned for future development"
#   (docs.fly.io/litefs/how-it-works). No configuration field waits for a replica
#   (cmd/litefs/config.go:28-42, 145-183).
# So the strongest LiteFS offers is the primary's own disk, fsynced: it can't match 2 of 3.
set -euo pipefail
SYS=litefs
PORT=${AMPHORA_BENCH_REPL_PORT:-54260}
DURABILITY="acknowledged once the primary's SQLite WAL and LTX file hold it, fsynced (synchronous=FULL; LiteFS's CommitWAL syncs both); replicas stream it asynchronously and LiteFS has no mode that waits for one, so no quorum: one member's disk"
# The comparability rule's column (lib.sh): a commit acknowledged by the primary alone; no mode waits for a replica (above).
COLUMN="primary only"
# The shim's own ready line (shim/main.go). LiteFS runs the shim only once its node holds the
# lease or has connected to the holder ("connected to cluster, ready",
# cmd/litefs/mount_linux.go:265-270), and passes its stdout and stderr on (mount_linux.go:617-623:
# the last exec entry runs in the background), so the line lands in the node's log.
READY='shim ready: listening on '
. "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
CONSUL_PORT=$((PORT + 20))
CONSUL_DIR="$DATA/consul"

before_up() {
  need_bin consul
  [[ -e /dev/fuse ]] || { echo "no /dev/fuse on this host: LiteFS can't mount" >&2; return 1; }
  command -v fusermount3 >/dev/null || { echo "no fusermount3 on this host: LiteFS can't mount" >&2; return 1; }
  mkdir -p "$CONSUL_DIR"
  setsid "$BIN/consul" agent -dev -bind 127.0.0.1 -client 127.0.0.1 -data-dir "$CONSUL_DIR/data" \
    -hcl "ports { http = $CONSUL_PORT, serf_lan = $((CONSUL_PORT + 1)), serf_wan = $((CONSUL_PORT + 2)), server = $((CONSUL_PORT + 3)), dns = -1, grpc = -1 }" \
    >>"$CONSUL_DIR/log" 2>&1 </dev/null &
  echo $! >"$CONSUL_DIR/pid"
  # Consul's own line once its server leads (agent/consul/leader.go:101 at v2.1.0: "cluster leadership
  # acquired"), so a LiteFS lease can be taken. Its log is new: DATA went with the last down.
  wait_line "$CONSUL_DIR/log" 0 "$(cat "$CONSUL_DIR/pid")" 'cluster leadership acquired' \
    || { echo "consul didn't take leadership; its log ends:" >&2; tail -n 40 "$CONSUL_DIR/log" >&2; return 1; }
}

after_kill() { fusermount3 -uz "$(dir "$1")/mnt" 2>/dev/null || true; }

after_down() {
  local f="$CONSUL_DIR/pid"
  [[ -f "$f" ]] && kill -KILL "$(cat "$f")" 2>/dev/null || true
}

run_node() {
  local n=$1 d
  need_bin litefs
  need_bin shim
  d=$(dir "$n")
  after_kill "$n"
  mkdir -p "$d/mnt" "$d/data"
  cat >"$d/litefs.yml" <<YML
fuse:
  dir: "$d/mnt"
data:
  dir: "$d/data"
http:
  addr: "127.0.0.1:$(port2 "$n")"
lease:
  type: "consul"
  hostname: "litefs-$n"
  advertise-url: "http://127.0.0.1:$(port2 "$n")"
  candidate: true
  consul:
    url: "http://127.0.0.1:$CONSUL_PORT"
    key: "litefs/$NAME"
exec:
  - cmd: "$BIN/shim -backend sqlite -name litefs-$n -path $d/mnt/bench.db -listen 127.0.0.1:$(port "$n")"
YML
  # The shim links libdqlite, which setup.sh may have built under $BIN/lib; LiteFS hands its own
  # environment to what it runs (cmd/litefs/mount_linux.go:621), as dqlite.sh's nodes have it.
  spawn "$n" env LD_LIBRARY_PATH="$BIN/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" "$BIN/litefs" mount -config "$d/litefs.yml"
}

leader() {
  local n name
  for n in 0 1 2; do
    name=$(curl -sf --max-time 2 "http://127.0.0.1:$(port "$n")/leader" 2>/dev/null || true)
    if [[ "$name" =~ -([0-9])$ ]]; then
      echo "${BASH_REMATCH[1]}"
      return 0
    fi
  done
}

version() { "$BIN/litefs" version 2>&1 || true; }

help_text() { "$BIN/litefs" mount -h; }

main "$@"
