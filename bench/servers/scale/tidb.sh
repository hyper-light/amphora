#!/usr/bin/env bash
# TiDB, one node a region: node n is PD n, TiKV n (label region=r<n>) and TiDB server n, for
# bench/src/scale.rs's `tidb` system: SQL on 127.0.0.1:PORT+n, status on PORT+100+n. A commit is acknowledged once a
# quorum of each Region's TiKV replicas holds it (Raft), by Percolator's two phases across Regions.
set -euo pipefail
SYS=tidb
VERSION=${TIDB_VERSION:-latest}
PD_IMAGE=pingcap/pd:$VERSION
TIKV_IMAGE=pingcap/tikv:$VERSION
TIDB_IMAGE=pingcap/tidb:$VERSION
IMAGES="$PD_IMAGE $TIKV_IMAGE $TIDB_IMAGE"
PORT=${AMPHORA_BENCH_SCALE_PORT:-56300}
. "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"

# Node n's TiDB status port on 127.0.0.1, past the SQL ports.
status_port() { echo $((PORT + 100 + $1)); }
containers() { echo "$(name "pd$1") $(name "tikv$1") $(name "tidb$1")"; }
pds() { local m s=""; for ((m = 0; m < $(nodes); m++)); do s+="${s:+,}$(name "pd$m"):2379"; done; echo "$s"; }

run_node() {
  local n=$1 m initial=""
  for ((m = 0; m < $(nodes); m++)); do initial+="${initial:+,}pd$m=http://$(name "pd$m"):2380"; done
  docker run -d --name "$(name "pd$n")" --hostname "$(name "pd$n")" --label "$LABEL" --network "$NET" "$PD_IMAGE" \
    --name="pd$n" --client-urls=http://0.0.0.0:2379 --peer-urls=http://0.0.0.0:2380 \
    --advertise-client-urls="http://$(name "pd$n"):2379" --advertise-peer-urls="http://$(name "pd$n"):2380" --initial-cluster="$initial"
  docker run -d --name "$(name "tikv$n")" --hostname "$(name "tikv$n")" --label "$LABEL" --network "$NET" "$TIKV_IMAGE" \
    --addr=0.0.0.0:20160 --advertise-addr="$(name "tikv$n"):20160" --status-addr=0.0.0.0:20180 --advertise-status-addr="$(name "tikv$n"):20180" \
    --pd="$(pds)" --labels "region=r$n" --data-dir=/data
  docker run -d --name "$(name "tidb$n")" --hostname "$(name "tidb$n")" --label "$LABEL" --network "$NET" -p "127.0.0.1:$(port "$n"):4000" -p "127.0.0.1:$(status_port "$n"):10080" "$TIDB_IMAGE" \
    --store=tikv --path="$(pds)" --advertise-address="$(name "tidb$n")" -P 4000 --status=10080
}

# PD places replicas by the region label: each Region's replicas in different regions.
after_up() {
  wait_for docker exec "$(name pd0)" /pd-ctl -u "http://$(name pd0):2379" config set location-labels region
}

# TiDB's status port answers once the server serves SQL.
ready_node() { curl -sf --max-time 2 "http://127.0.0.1:$(status_port "$1")/status" >/dev/null; }

endpoint() { echo "mysql://root@127.0.0.1:$(port "$1")/test"; }

main "$@"
