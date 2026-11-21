# Sourced by the three-node cluster scripts beside it (bench/src/repl.rs drives them). Each sets SYS,
# IMAGE and PORT (the first node's HTTP port on 127.0.0.1; node n takes PORT+n), defines
# `run_node N` (its `docker run -d` on the cluster's network), `ready_node N` (true once node N
# answers) and `leader` (prints the leader's node number, 0 to 2, or nothing), then calls
# `main "$@"`.
#
# The nodes share a docker network of their own, amphora-bench-repl-<system>, and reach each other
# by container name; each publishes its HTTP port on 127.0.0.1 only. Their data stays inside the
# containers, so `down` (docker rm -f -v) leaves nothing on the host but the pulled or built image:
# `images` prints its ID for removal by ID. Every container and network is named
# amphora-bench-repl-<system>[-N]: only those are ever stopped or removed.
#
# usage: <system>.sh up|down|kill N|start N|leader|endpoint N|images
set -euo pipefail
NET="amphora-bench-repl-$SYS"
node() { echo "amphora-bench-repl-$SYS-$1"; }
port() { echo $((PORT + $1)); }

# Polls `$@` every half second at most 240 times (two minutes, lib.sh's bound for a first start).
wait_for() {
  local i
  for ((i = 0; i < 240; i++)); do
    if "$@" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.5
  done
  return 1
}

down() {
  local n
  for n in 0 1 2; do
    docker rm -f -v "$(node "$n")" >/dev/null 2>&1 || true
  done
  docker network rm "$NET" >/dev/null 2>&1 || true
}

up() {
  down
  docker network create "$NET" >/dev/null
  local n
  for n in 0 1 2; do
    run_node "$n" >/dev/null
  done
  for n in 0 1 2; do
    if ! wait_for ready_node "$n"; then
      echo "$(node "$n") didn't come up; its log ends:" >&2
      docker logs --tail 40 "$(node "$n")" >&2 || true
      return 1
    fi
  done
  if ! wait_for has_leader; then
    echo "$SYS named no leader" >&2
    return 1
  fi
}

has_leader() { [[ -n "$(leader)" ]]; }

main() {
  case "${1:-}" in
    up) up ;;
    down) down ;;
    kill) docker kill --signal KILL "$(node "$2")" >/dev/null ;;
    start)
      docker start "$(node "$2")" >/dev/null
      wait_for ready_node "$2"
      ;;
    leader) leader ;;
    endpoint) echo "http://127.0.0.1:$(port "$2")" ;;
    images) docker image inspect -f '{{.Id}}' "$IMAGE" ;;
    *)
      echo "usage: $0 up|down|kill N|start N|leader|endpoint N|images" >&2
      return 2
      ;;
  esac
}
