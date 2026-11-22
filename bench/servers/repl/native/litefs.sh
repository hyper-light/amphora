#!/usr/bin/env bash
# LiteFS, three candidates for one Consul lease, each `litefs mount` a process on this host
# (setup.sh's release binaries), SQLite on each node's FUSE mount through the bench's shim
# (../shim/main.go, which LiteFS runs once the node is primary or follows one) on
# 127.0.0.1:54260-54262; LiteFS's own HTTP on 54270-54272. The lease's holder is the primary; a
# commit is acknowledged once the primary's SQLite holds it (synchronous=FULL), and replicas stream it
# after (LiteFS replicates asynchronously). A write to a replica fails read-only, so the harness sends
# it on to the next node. Consul runs in dev mode beside the nodes (HTTP on 54280, its other ports
# 54281-54283, DNS off), and goes with them. FUSE needs /dev/fuse and fusermount3 on the host.
set -euo pipefail
SYS=litefs
PORT=${AMPHORA_BENCH_REPL_PORT:-54260}
DURABILITY="acknowledged once the primary's SQLite WAL holds it on disk (synchronous=FULL); replicas stream it asynchronously, so no quorum: one member's disk"
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
  wait_for curl -sf "http://127.0.0.1:$CONSUL_PORT/v1/status/leader"
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
  spawn "$n" "$BIN/litefs" mount -config "$d/litefs.yml"
}

ready_node() { curl -sf "http://127.0.0.1:$(port "$1")/ready" >/dev/null; }

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

main "$@"
