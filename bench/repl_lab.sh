#!/usr/bin/env bash
# Replication measured, the three-machine lab (CP-T1.8; G12 steps 1 and 3-5; AC-T1.8.5's 10,000
# seeds in both commit classes). Run from a checkout of the commit to measure; the three machines
# are its arguments, reached by ssh, each with a clone of this repository at LAB_REPO (default the
# same path relative to its home as this checkout's to this home), bash, cargo and, for the
# comparison set, docker and go. Nothing here picks a machine: the person running it names them.
#
# usage: bench/repl_lab.sh RUN HOST1 HOST2 HOST3 [-- REPL_FLAG VALUE ...]
#   RUN       the run's name: its records go to bench/records/power-nemesis/RUN/ and
#             bench/records/repl/RUN/<host>/ in this checkout, to be committed.
#   HOSTn     ssh destinations.
#   REPL_FLAGs bench/repl.sh's flags (amphora-bench --repl's: every size and duration, none picked
#             here); without them the comparison set is skipped.
# Environment:
#   SEEDS     the nemesis seeds a class (default 10000, CP-T1.8's count), split in three ranges,
#             one a machine, each run with bench/power_nemesis.sh; then merged per class.
#   CLASSES   (default "replicated persisted"). STEPS (default "nemesis repl").
#   SYSTEMS, MEASURES: passed to bench/repl.sh on every machine.
# Amphora's three members on three machines (the LAN p99) are bench/repl_members.sh's, each member
# its own process at an address given as an argument.
# Exit: 0 when every step on every machine gave its records and both merges passed; 1 otherwise.
set -uo pipefail
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd -- "$HERE/.." && pwd)"
[[ $# -ge 4 ]] || { echo "usage: $0 RUN HOST1 HOST2 HOST3 [-- REPL_FLAG VALUE ...]" >&2; exit 2; }
RUN=$1 HOSTS=("$2" "$3" "$4")
shift 4
[[ "${1:-}" == "--" ]] && shift
FLAGS=("$@")
SEEDS=${SEEDS:-10000}
CLASSES=${CLASSES:-replicated persisted}
STEPS=${STEPS:-nemesis repl}
COMMIT=$(git -C "$ROOT" rev-parse HEAD)
LAB_REPO=${LAB_REPO:-${ROOT#"$HOME"/}}
NEMESIS="$ROOT/bench/records/power-nemesis/$RUN"
REPL="$ROOT/bench/records/repl/$RUN"
q() { printf '%q ' "$@"; }

# Every machine at this commit, the commit fetched from origin (push it first).
at_commit() {
  local h=$1
  ssh "$h" "cd $(q "$LAB_REPO") && git fetch -q origin && git checkout -q --detach $COMMIT && git rev-parse HEAD" | grep -qx "$COMMIT"
}

failed=0
for h in "${HOSTS[@]}"; do
  at_commit "$h" || { echo "$h: can't check out $COMMIT in $LAB_REPO" >&2; exit 1; }
done

if [[ " $STEPS " == *" nemesis "* ]]; then
  mkdir -p "$NEMESIS"
  for class in $CLASSES; do
    pids=()
    for i in 0 1 2; do
      from=$((SEEDS * i / 3)) to=$((SEEDS * (i + 1) / 3))
      h=${HOSTS[$i]}
      remote="bench/records/power-nemesis/$RUN"
      (
        ssh "$h" "cd $(q "$LAB_REPO") && bench/power_nemesis.sh run $class $from $((to - from)) $(q "$remote")"
        status=$?
        scp -q "$h:$LAB_REPO/$remote/$class-$from-$((to - from)).json" "$NEMESIS/" || status=1
        exit $status
      ) &
      pids+=($!)
    done
    for p in "${pids[@]}"; do wait "$p" || failed=1; done
    "$ROOT/bench/power_nemesis.sh" merge "$NEMESIS" "$class" "$SEEDS" || failed=1
  done
fi

if [[ " $STEPS " == *" repl "* && ${#FLAGS[@]} -gt 0 ]]; then
  # One machine at a time: the comparison set's clusters each take a machine's cores and disk.
  for h in "${HOSTS[@]}"; do
    remote="bench/records/repl/$RUN"
    ssh "$h" "cd $(q "$LAB_REPO") && SYSTEMS=$(q "${SYSTEMS:-}") MEASURES=$(q "${MEASURES:-}") bench/repl.sh $(q "$remote") $(q "${FLAGS[@]}")" || failed=1
    mkdir -p "$REPL/$h"
    scp -q "$h:$LAB_REPO/$remote/*.jsonl" "$REPL/$h/" || failed=1
  done
fi
exit $failed
