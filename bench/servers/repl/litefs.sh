#!/usr/bin/env bash
# LiteFS, three candidates for one Consul lease, for bench/src/repl.rs's `litefs` system: SQLite on
# each node's LiteFS mount through the bench's shim (shim/main.go, image amphora-bench-repl-shim) on
# 127.0.0.1:54130-54132. The lease's holder is the primary; a commit is acknowledged once the
# primary's SQLite holds it, and replicas stream it after (LiteFS replicates asynchronously). A
# write to a replica fails read-only, so the harness sends it on to the next node. When the primary
# dies its Consul session expires and another candidate takes the lease: that is the failover the
# harness times. FUSE needs --privileged. Consul runs in dev mode beside the nodes, as
# amphora-bench-repl-litefs-consul, and goes with them.
set -euo pipefail
SYS=litefs
IMAGE=${IMAGE:-amphora-bench-repl-shim:local}
CONSUL_IMAGE=${CONSUL_IMAGE:-hashicorp/consul:latest}
PORT=${AMPHORA_BENCH_REPL_PORT:-54130}
HERE="$(dirname -- "${BASH_SOURCE[0]}")"
. "$HERE/lib.sh"
CONSUL="amphora-bench-repl-$SYS-consul"
EXTRA_IMAGE=$CONSUL_IMAGE

before_up() {
  docker image inspect "$IMAGE" >/dev/null 2>&1 || docker build -q -t "$IMAGE" "$HERE/shim" >/dev/null
  docker run -d --name "$CONSUL" --network "$NET" "$CONSUL_IMAGE" agent -dev -client 0.0.0.0 >/dev/null
  wait_for docker exec "$CONSUL" consul members
}

after_down() { docker rm -f -v "$CONSUL" >/dev/null 2>&1 || true; }

run_node() {
  local n=$1
  docker run -d --name "$(node "$n")" --hostname "$(node "$n")" --network "$NET" --privileged -p "127.0.0.1:$(port "$n"):8080" \
    -e LITEFS_HOSTNAME="$(node "$n")" -e LITEFS_CONSUL="http://$CONSUL:8500" "$IMAGE" litefs mount -config /etc/litefs.yml
}

ready_node() { curl -sf "http://127.0.0.1:$(port "$1")/ready" >/dev/null; }

# The lease's holder as any live node's shim says it (LiteFS's .primary file), as its node number.
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

main "$@"
