# Sourced by the cluster scripts beside it (bench/src/scale.rs drives them; CP-T1.9 to CP-T1.11). Each
# sets SYS, IMAGES (every image its nodes run) and PORT (node n's SQL port on 127.0.0.1 is PORT+n),
# defines `containers N` (node N's containers, by name), `run_node N` (its `docker run -d`s on the
# cluster's network), `ready_node N` (true once node N answers SQL), `endpoint N` (its client URL),
# and optionally `after_up` (run once every node is up), then calls `main "$@"`.
#
# Node n is region r<n>. Every container carries the label amphora-bench-scale=<system>, and is
# stopped and removed only by the IDs that label finds; `images` prints the images' IDs for removal
# by ID. Data stays inside the containers: `down` (docker rm -f -v) leaves only the pulled images.
#
# `delay MS`: MS milliseconds one way on every packet a node's containers send to another node's
# (netem on a prio band its peers' addresses are filtered to: client traffic from the host isn't
# delayed), from NETEM_IMAGE run in each container's network namespace; `delay 0` clears it.
#
# Waits are bounded by WAIT_S seconds (the driver passes its --deadline-s), polled every POLL_S.
#
# usage: <system>.sh up N|down|kill N|start N|endpoint N|delay MS|images|ids
set -euo pipefail
# Git for Windows' bash rewrites arguments that look like paths (/data, /etc/...): not the containers'.
export MSYS_NO_PATHCONV=1
NET="amphora-bench-scale-$SYS"
LABEL="amphora-bench-scale=$SYS"
NETEM_IMAGE=${NETEM_IMAGE:-nicolaka/netshoot:latest}
WAIT_S=${WAIT_S:?WAIT_S: the driver's deadline in seconds}
POLL_S=${POLL_S:-1}
STATE="${XDG_CACHE_HOME:-$HOME/.cache}/amphora-bench-scale-$SYS.nodes"
name() { echo "amphora-bench-scale-$SYS-$1"; }
port() { echo $((PORT + $1)); }
nodes() { cat "$STATE" 2>/dev/null || echo 0; }

wait_for() {
  local until=$((SECONDS + WAIT_S))
  while ((SECONDS < until)); do
    if "$@" >/dev/null 2>&1; then
      return 0
    fi
    sleep "$POLL_S"
  done
  return 1
}

ids() { docker ps -aq --filter "label=$LABEL"; }

down() {
  local id
  for id in $(ids); do
    docker rm -f -v "$id" >/dev/null 2>&1 || true
  done
  docker network rm "$NET" >/dev/null 2>&1 || true
  rm -f "$STATE"
}

up() {
  local n count=${1:?up takes the node count}
  down
  echo "$count" >"$STATE"
  docker network create "$NET" >/dev/null
  for ((n = 0; n < count; n++)); do
    run_node "$n" >/dev/null
  done
  if declare -F after_up >/dev/null; then after_up; fi
  for ((n = 0; n < count; n++)); do
    if ! wait_for ready_node "$n"; then
      echo "node $n didn't come up within ${WAIT_S}s; its logs end:" >&2
      local c
      for c in $(containers "$n"); do docker logs --tail 30 "$c" >&2 || true; done
      return 1
    fi
  done
}

addr() { docker inspect -f "{{(index .NetworkSettings.Networks \"$NET\").IPAddress}}" "$1"; }

delay() {
  local ms=${1:?delay takes milliseconds} n m c p count
  count=$(nodes)
  for ((n = 0; n < count; n++)); do
    for c in $(containers "$n"); do
      local tc="tc qdisc del dev eth0 root 2>/dev/null || true;"
      if [[ "$ms" != "0" ]]; then
        tc+=" tc qdisc add dev eth0 root handle 1: prio && tc qdisc add dev eth0 parent 1:3 handle 30: netem delay ${ms}ms"
        for ((m = 0; m < count; m++)); do
          ((m == n)) && continue
          for p in $(containers "$m"); do
            tc+=" && tc filter add dev eth0 protocol ip parent 1:0 prio 3 u32 match ip dst $(addr "$p")/32 flowid 1:3"
          done
        done
      fi
      docker run --rm --label "$LABEL" --network "container:$c" --cap-add NET_ADMIN --entrypoint sh "$NETEM_IMAGE" -c "$tc" >/dev/null
    done
  done
}

main() {
  case "${1:-}" in
    up) up "${2:-}" ;;
    down) down ;;
    kill) for c in $(containers "$2"); do docker kill --signal KILL "$c" >/dev/null; done ;;
    start)
      for c in $(containers "$2"); do docker start "$c" >/dev/null; done
      wait_for ready_node "$2"
      ;;
    endpoint) endpoint "$2" ;;
    delay) delay "$2" ;;
    images) docker image inspect -f '{{.Id}}' $IMAGES "$NETEM_IMAGE" 2>/dev/null || true ;;
    ids) ids ;;
    *)
      echo "usage: $0 up N|down|kill N|start N|endpoint N|delay MS|images|ids" >&2
      return 2
      ;;
  esac
}
