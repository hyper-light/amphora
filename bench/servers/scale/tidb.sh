#!/usr/bin/env bash
# TiDB, natively or in containers (lib.sh's MODE), one node a region: node n is PD n, TiKV n (label region=r<n>) and TiDB server n, for
# bench/src/scale.rs's `tidb` system, on the machines HOSTS names (lib.sh). Node n's ports from
# BASE + 6n: PD client, PD peer, TiKV, TiKV status, TiDB SQL, TiDB status. A commit is acknowledged
# once a quorum of each Region's TiKV replicas holds it (Raft), by Percolator's two phases across
# Regions.
set -euo pipefail
SYS=tidb
VERSION=${TIDB_VERSION:-latest}
PD_IMAGE=pingcap/pd:$VERSION
TIKV_IMAGE=pingcap/tikv:$VERSION
TIDB_IMAGE=pingcap/tidb:$VERSION
IMAGES="$PD_IMAGE $TIKV_IMAGE $TIDB_IMAGE"
PORTS=6
BASE=56300
. "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
[[ $MODE == docker ]] || BIN=${AMPHORA_BENCH_SCALE_BIN:?AMPHORA_BENCH_SCALE_BIN: the folder holding the TiDB release binaries (bench/servers/scale/fetch.sh)}

at() { echo "$(addr "$1"):$(port "$1" "$2")"; }
containers() { echo "$(name "pd$1") $(name "tikv$1") $(name "tidb$1")"; }
pds() { local m s=""; for ((m = 0; m < $(nodes); m++)); do s+="${s:+,}$(at "$m" 0)"; done; echo "$s"; }

run_node() {
  local n=$1 m initial=""
  for ((m = 0; m < $(nodes); m++)); do initial+="${initial:+,}pd$m=http://$(at "$m" 1)"; done
  dk "$n" run -d --name "$(name "pd$n")" --label "$LABEL" -p "$(port "$n" 0):$(port "$n" 0)" -p "$(port "$n" 1):$(port "$n" 1)" "$PD_IMAGE" \
    --name="pd$n" --client-urls="http://0.0.0.0:$(port "$n" 0)" --peer-urls="http://0.0.0.0:$(port "$n" 1)" \
    --advertise-client-urls="http://$(at "$n" 0)" --advertise-peer-urls="http://$(at "$n" 1)" --initial-cluster="$initial"
  dk "$n" run -d --name "$(name "tikv$n")" --label "$LABEL" -p "$(port "$n" 2):$(port "$n" 2)" -p "$(port "$n" 3):$(port "$n" 3)" "$TIKV_IMAGE" \
    --addr="0.0.0.0:$(port "$n" 2)" --advertise-addr="$(at "$n" 2)" --status-addr="0.0.0.0:$(port "$n" 3)" --advertise-status-addr="$(at "$n" 3)" \
    --pd="$(pds)" --labels "region=r$n" --data-dir=/data
  dk "$n" run -d --name "$(name "tidb$n")" --label "$LABEL" -p "$(port "$n" 4):$(port "$n" 4)" -p "$(port "$n" 5):$(port "$n" 5)" "$TIDB_IMAGE" \
    --store=tikv --path="$(pds)" --advertise-address="$(addr "$n")" -P "$(port "$n" 4)" --status="$(port "$n" 5)"
}

# Native mode: BIN holds the release's pd-server, tikv-server, tidb-server and pd-ctl
# (bench/servers/scale/fetch.sh); node n's PD and TiKV data in its folder. TiKV's block cache is
# its own default share of memory (45%, TiKV's storage.block-cache.capacity) of this node's part of
# the machine, the machine's memory split evenly over the nodes it runs: TiKV's default takes the
# whole machine's, which nodes sharing it can't all have.
run_native() {
  local n=$1 m initial="" here=0 kb
  for ((m = 0; m < $(nodes); m++)); do
    initial+="${initial:+,}pd$m=http://$(at "$m" 1)"
    [[ $(machine "$m") == "$(machine "$n")" ]] && here=$((here + 1))
  done
  kb=$(on_node "$n" "sed -n 's/^MemTotal: *\([0-9]*\) kB/\1/p' /proc/meminfo")
  on_node "$n" "mkdir -p $(printf %q "$RUNDIR/$n") && printf '[storage.block-cache]\ncapacity = \"%sKB\"\n' $((kb * 45 / 100 / here)) >$(printf %q "$RUNDIR/$n/tikv.toml")"
  proc "$n" pd "$BIN/pd-server" --name="pd$n" --client-urls="http://0.0.0.0:$(port "$n" 0)" --peer-urls="http://0.0.0.0:$(port "$n" 1)" \
    --advertise-client-urls="http://$(at "$n" 0)" --advertise-peer-urls="http://$(at "$n" 1)" --initial-cluster="$initial" --data-dir="$RUNDIR/$n/pd"
  proc "$n" tikv "$BIN/tikv-server" --addr="0.0.0.0:$(port "$n" 2)" --advertise-addr="$(at "$n" 2)" --status-addr="0.0.0.0:$(port "$n" 3)" \
    --advertise-status-addr="$(at "$n" 3)" --pd="$(pds)" --labels "region=r$n" --data-dir="$RUNDIR/$n/tikv" --config="$RUNDIR/$n/tikv.toml"
  proc "$n" tidb "$BIN/tidb-server" --store=tikv --path="$(pds)" --advertise-address="$(addr "$n")" -P "$(port "$n" 4)" --status="$(port "$n" 5)"
}

# pd-ctl through node n's PD.
node_exec() {
  local n=$1
  shift
  if [[ $MODE == native ]]; then
    "$BIN/pd-ctl" -u "http://$(at "$n" 0)" "$@"
  else
    dk "$n" exec "$(name "pd$n")" /pd-ctl -u "http://$(at "$n" 0)" "$@"
  fi
}

# PD places replicas by the region label: each Region's replicas in different regions.
after_up() {
  wait_for node_exec 0 config set location-labels region
}

# TiDB's status port answers once the server serves SQL.
ready_node() { curl -sf --max-time 2 "http://$(at "$1" 5)/status" >/dev/null; }

endpoint() { echo "mysql://root@$(at "$1" 4)/test"; }

# The TiKV client's Region errors among them (bench/src/scale.rs's stale routes).
metrics_url() { echo "http://$(at "$1" 5)/metrics"; }

main "$@"
