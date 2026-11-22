#!/usr/bin/env bash
# Amphora as its own server (CP-T1.13's `amphora serve`), one member a node, for bench/src/scale.rs's
# `amphora` system, on the machines HOSTS names (lib.sh, native mode only): node n is member n+1 of
# one replicated database, its peers and its clients on its machine's address at port BASE + n, its
# data in its folder. Reached by the bench over TCP like CockroachDB and TiDB are: the server's
# client protocol (a line a request, after the cluster's secret is proved). A commit is
# acknowledged once a quorum of the members holds it (Raft). BIN holds the `amphora` binary built
# from the branch that has `serve` (cargo build --release -p amphora). CLASS is the members'
# durability class (persisted, or replicated with SPANNING), DEADLINE_MS their bound on one request:
# the bench passes its own.
set -euo pipefail
SYS=amphora
IMAGES=""
PORTS=1
BASE=56500
. "$(dirname -- "${BASH_SOURCE[0]}")/lib.sh"
[[ $MODE == native ]] || { echo "amphora.sh runs Amphora's server natively only (AMPHORA_BENCH_SCALE_MODE=native)" >&2; exit 2; }
BIN=${AMPHORA_BENCH_SCALE_AMPHORA:?AMPHORA_BENCH_SCALE_AMPHORA: the amphora binary that serves (cargo build --release -p amphora)}
CLASS=${CLASS:?CLASS: persisted or replicated}
DEADLINE_MS=${DEADLINE_MS:?DEADLINE_MS: a request's bound, the bench's --deadline-s}
SECRET="$RUNDIR/secret"

containers() { :; }
at() { echo "$(addr "$1"):$(port "$1" 0)"; }

run_native() {
  local n=$1 m members=""
  for ((m = 0; m < $(nodes); m++)); do members+="${members:+,}$((m + 1))@$(at "$m")"; done
  # The cluster's secret: 32 random bytes, hex (RFC 2104 §3's least for HMAC-SHA256), the same file on every machine.
  [[ -s $SECRET ]] || { mkdir -p "$RUNDIR" && od -An -tx1 -N32 /dev/urandom | tr -d ' \n' >"$SECRET"; }
  on_node "$n" "mkdir -p $(printf %q "$RUNDIR/$n") && { [[ -s $(printf %q "$SECRET") ]] || cat >$(printf %q "$SECRET"); } && cat >$(printf %q "$RUNDIR/$n/member.conf") <<CONF
node = $((n + 1))
members = $members
data = $RUNDIR/$n/data
class = $CLASS
${SPANNING:+spanning = $SPANNING}
secret_file = $SECRET
deadline_ms = $DEADLINE_MS
CONF" <"$SECRET"
  proc "$n" amphora "$BIN" serve "$RUNDIR/$n/member.conf"
}

node_exec() {
  local n=$1
  shift
  "$BIN" ask "$RUNDIR/$n/member.conf" "$((n + 1))" "$@"
}

ready_node() { node_exec "$1" status | grep -q '^ok '; }

endpoint() { echo "$(at "$1") $SECRET"; }

metrics_url() { echo ""; }

main "$@"
