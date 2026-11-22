# Sourced by the cluster scripts beside it (bench/src/scale.rs drives them; CP-T1.9 to CP-T1.11). Each
# sets SYS, IMAGES (every image its nodes run) and PORTS (how many ports a node takes), defines
# `containers N` (node N's containers, by name), `run_node N` (its `docker run -d`s), `ready_node N`
# (true once node N answers SQL), `endpoint N` (its client URL), and optionally `after_up` (run once
# every node is up), then calls `main "$@"`.
#
# The machines are arguments, never names in code: HOSTS is a comma-separated list of
# ENDPOINT=ADDR, one a machine. ENDPOINT is the Docker daemon there (`local` for this one's, else
# anything `docker -H` takes: ssh://user@host, tcp://host:2376); ADDR is the address every other
# machine and the bench reach that machine by. Node n runs on machine n mod the machines' count, so
# five nodes on five machines is one node a machine, and the same five on one machine is a lab.
#
# Node n takes ports BASE + n*PORTS .. BASE + n*PORTS + PORTS-1 on its machine (BASE:
# AMPHORA_BENCH_SCALE_PORT, else the script's own), published as the same numbers inside its
# containers, so the address it advertises is the address its peers dial: one path whether the
# nodes share a machine or not. Node n is region r<n>. Every container carries the label
# amphora-bench-scale=<system>, and is stopped and removed only by the IDs that label finds on each
# machine; `rmi` removes the images they ran, by ID, on each machine. Data stays inside the containers:
# `down` (docker rm -f -v) leaves only the pulled images.
#
# `delay MS`: MS milliseconds one way on every packet a node's containers send to another node's
# ports (netem on a prio band its peers' address-and-port pairs are filtered to: client traffic from
# the bench isn't delayed), from NETEM_IMAGE run in each container's network namespace; `delay 0`
# clears it.
#
# Waits are bounded by WAIT_S seconds (the driver passes its --deadline-s), polled every POLL_S.
#
# usage: HOSTS=... <system>.sh up N|down|kill N|start N|endpoint N|delay MS|rmi|ids|exec N ARGS...
set -euo pipefail
# Git for Windows' bash rewrites arguments that look like paths (/data, /etc/...): not the containers'.
export MSYS_NO_PATHCONV=1
# RUN (AMPHORA_BENCH_SCALE_RUN) names this run's containers apart from other runs sharing a Docker
# daemon: they carry it in their names, label and state file, so `down` removes only this run's.
RUN=${AMPHORA_BENCH_SCALE_RUN:-}
LABEL="amphora-bench-scale=$SYS${RUN:+-$RUN}"
NETEM_IMAGE=${NETEM_IMAGE:-nicolaka/netshoot:latest}
WAIT_S=${WAIT_S:?WAIT_S: the deadline in seconds the driver passes}
POLL_S=${POLL_S:-1}
HOSTS=${HOSTS:?HOSTS: the machines, ENDPOINT=ADDR,... (ENDPOINT local or a docker -H endpoint)}
BASE=${AMPHORA_BENCH_SCALE_PORT:-$BASE}
STATE="${XDG_CACHE_HOME:-$HOME/.cache}/amphora-bench-scale-$SYS${RUN:+-$RUN}.nodes"
IFS=, read -r -a MACHINES <<<"$HOSTS"
((${#MACHINES[@]} > 0)) || { echo "HOSTS names no machine" >&2; exit 2; }
for spec in "${MACHINES[@]}"; do
  [[ $spec == *=?* && ${spec%%=*} != "" ]] || { echo "HOSTS entry '$spec' isn't ENDPOINT=ADDR" >&2; exit 2; }
done
name() { echo "amphora-bench-scale-$SYS${RUN:+-$RUN}-$1"; }
nodes() { cat "$STATE" 2>/dev/null || echo 0; }
machine() { echo "${MACHINES[$(($1 % ${#MACHINES[@]}))]}"; }
# Node n's machine's address.
addr() { local m; m=$(machine "$1"); echo "${m#*=}"; }
# Node n's j-th port.
port() { echo $((BASE + $1 * PORTS + $2)); }
# docker on node n's machine.
dk() {
  local n=$1 m ep
  shift
  m=$(machine "$n")
  ep=${m%%=*}
  if [[ $ep == local ]]; then docker "$@"; else docker -H "$ep" "$@"; fi
}
# -p for each of node n's ports, the same number inside.
publish() {
  local n=$1 j p
  for ((j = 0; j < PORTS; j++)); do
    p=$(port "$n" "$j")
    printf -- '-p %s:%s ' "$p" "$p"
  done
}

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

# Each machine once: the first node on it stands for it.
each_machine() {
  local i
  for ((i = 0; i < ${#MACHINES[@]}; i++)); do "$@" "$i"; done
}
ids_on() { dk "$1" ps -aq --filter "label=$LABEL"; }
ids() { each_machine ids_on; }
down_on() {
  local id
  for id in $(ids_on "$1"); do
    dk "$1" rm -f -v "$id" >/dev/null 2>&1 || true
  done
}
down() {
  each_machine down_on
  rm -f "$STATE"
}

up() {
  local n count=${1:?up takes the node count}
  down
  echo "$count" >"$STATE"
  for ((n = 0; n < count; n++)); do
    run_node "$n" >/dev/null
  done
  if declare -F after_up >/dev/null; then after_up; fi
  for ((n = 0; n < count; n++)); do
    if ! wait_for ready_node "$n"; then
      echo "node $n didn't come up within ${WAIT_S}s; its logs end:" >&2
      local c
      for c in $(containers "$n"); do dk "$n" logs --tail 30 "$c" >&2 || true; done
      return 1
    fi
  done
}

delay() {
  local ms=${1:?delay takes milliseconds} n m j c count
  count=$(nodes)
  for ((n = 0; n < count; n++)); do
    for c in $(containers "$n"); do
      local tc="tc qdisc del dev eth0 root 2>/dev/null || true;"
      if [[ "$ms" != "0" ]]; then
        tc+=" tc qdisc add dev eth0 root handle 1: prio && tc qdisc add dev eth0 parent 1:3 handle 30: netem delay ${ms}ms"
        for ((m = 0; m < count; m++)); do
          ((m == n)) && continue
          for ((j = 0; j < PORTS; j++)); do
            tc+=" && tc filter add dev eth0 protocol ip parent 1:0 prio 3 u32 match ip dst $(addr "$m")/32 match ip dport $(port "$m" "$j") 0xffff flowid 1:3"
          done
        done
      fi
      dk "$n" run --rm --label "$LABEL" --network "container:$c" --cap-add NET_ADMIN --entrypoint sh "$NETEM_IMAGE" -c "$tc" >/dev/null
    done
  done
}

# The images the nodes ran, removed by ID on each machine.
rmi_on() {
  local id
  for id in $(dk "$1" image inspect -f '{{.Id}}' $IMAGES "$NETEM_IMAGE" 2>/dev/null || true); do
    dk "$1" image rm "$id" >/dev/null 2>&1 || true
  done
}

main() {
  case "${1:-}" in
    up) up "${2:-}" ;;
    down) down ;;
    kill) for c in $(containers "$2"); do dk "$2" kill --signal KILL "$c" >/dev/null; done ;;
    start)
      for c in $(containers "$2"); do dk "$2" start "$c" >/dev/null; done
      wait_for ready_node "$2"
      ;;
    endpoint) endpoint "$2" ;;
    delay) delay "$2" ;;
    rmi) each_machine rmi_on ;;
    ids) ids ;;
    exec)
      local n=$2
      shift 2
      node_exec "$n" "$@"
      ;;
    *)
      echo "usage: HOSTS=ENDPOINT=ADDR,... $0 up N|down|kill N|start N|endpoint N|delay MS|rmi|ids|exec N ARGS..." >&2
      return 2
      ;;
  esac
}
