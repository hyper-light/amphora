#!/usr/bin/env bash
# LiteFS, one primary and two replicas on its static lease, for bench/src/repl.rs's `litefs` system:
# SQLite on each node's LiteFS mount through the bench's shim (shim/main.go, image
# amphora-bench-repl-shim) on 127.0.0.1:54130-54132. A commit is acknowledged once the primary's
# SQLite holds it; replicas stream it after (LiteFS replicates asynchronously). A write to a replica
# fails read-only, so the harness sends it on to the next node. FUSE needs --privileged.
set -euo pipefail
SYS=litefs
IMAGE=${IMAGE:-amphora-bench-repl-shim:local}
PORT=${AMPHORA_BENCH_REPL_PORT:-54130}
HERE="$(dirname -- "${BASH_SOURCE[0]}")"
. "$HERE/lib.sh"

run_node() {
  local n=$1 candidate=false
  [[ "$n" == 0 ]] && candidate=true
  docker image inspect "$IMAGE" >/dev/null 2>&1 || docker build -q -t "$IMAGE" "$HERE/shim" >/dev/null
  docker run -d --name "$(node "$n")" --network "$NET" --privileged -p "127.0.0.1:$(port "$n"):8080" \
    -e LITEFS_CANDIDATE="$candidate" -e LITEFS_PRIMARY="http://$(node 0):20202" "$IMAGE" litefs mount -config /etc/litefs.yml
}

ready_node() { curl -sf "http://127.0.0.1:$(port "$1")/ready" >/dev/null; }

leader() { ready_node 0 && echo 0; }

main "$@"
