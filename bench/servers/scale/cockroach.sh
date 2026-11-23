#!/usr/bin/env bash
# CockroachDB, one node a region (--locality=region=r<n>), insecure, natively or in containers (lib.sh's MODE), for bench/src/scale.rs's
# `cockroach` system, on the machines HOSTS names (lib.sh): node n's SQL and RPC on its machine's
# address at port BASE + 2n, its HTTP at BASE + 2n + 1. A commit is acknowledged once a quorum of its
# range's replicas holds it (Raft), with Parallel Commits across ranges.
set -euo pipefail
SYS=cockroach
IMAGE=${IMAGE:-cockroachdb/cockroach:latest}
IMAGES=$IMAGE
PORTS=2
BASE=56200
. "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
[[ $MODE == docker ]] || BIN=${AMPHORA_BENCH_SCALE_BIN:?AMPHORA_BENCH_SCALE_BIN: the folder holding the cockroach release binary (bench/servers/scale/fetch.sh)}

containers() { name "$1"; }
sql_at() { echo "$(addr "$1"):$(port "$1" 0)"; }

run_node() {
  local n=$1 join="" m
  for ((m = 0; m < $(nodes); m++)); do join+="${join:+,}$(sql_at "$m")"; done
  # shellcheck disable=SC2046 # publish's words are the -p flags
  dk "$n" run -d --name "$(name "$n")" --hostname "$(name "$n")" --label "$LABEL" $(publish "$n") "$IMAGE" \
    start --insecure --join="$join" --advertise-addr="$(sql_at "$n")" --listen-addr="0.0.0.0:$(port "$n" 0)" --http-addr="0.0.0.0:$(port "$n" 1)" --locality="region=r$n"
}

# Native mode: BIN holds the release's `cockroach` (bench/servers/scale/fetch.sh); node n's store in
# its folder.
run_native() {
  local n=$1 join="" m
  for ((m = 0; m < $(nodes); m++)); do join+="${join:+,}$(sql_at "$m")"; done
  proc "$n" cockroach "$BIN/cockroach" start --insecure --join="$join" --advertise-addr="$(sql_at "$n")" --listen-addr="0.0.0.0:$(port "$n" 0)" \
    --http-addr="0.0.0.0:$(port "$n" 1)" --locality="region=r$n" --store="path=$RUNDIR/$n/store"
}

node_exec() {
  local n=$1
  shift
  if [[ $MODE == native ]]; then
    "$BIN/cockroach" "$@" --insecure --host="$(sql_at "$n")"
  else
    dk "$n" exec "$(name "$n")" ./cockroach "$@" --insecure --host="$(sql_at "$n")"
  fi
}

after_up() {
  wait_for node_exec 0 init || node_exec 0 node status >/dev/null
}

ready_node() { node_exec "$1" sql -e "SELECT 1" >/dev/null; }

endpoint() { echo "host=$(addr "$1") port=$(port "$1" 0) user=root dbname=defaultdb sslmode=disable"; }

# The DistSender's counters among them (bench/src/scale.rs's stale routes).
metrics_url() { echo "http://$(addr "$1"):$(port "$1" 1)/_status/vars"; }

main "$@"
