#!/usr/bin/env bash
# The owner's stalls and leader changes, one commit against another (kiln goal ab3aedaf: leader-election churn
# around the persisted log's flush, recorded against the commit before it; goal aa81fbbf, AC-T1.8.D2, T-T1.8.D2;
# docs/research/owner-stalls.md, "An open liveness cost: terms that still move").
#
# Each commit's amphora lib tests are built in its own worktree and target directory (bench/ab_build.sh), then
# database::replicated::tests::production_owner_stalls_measured_on_this_host runs for <seconds> per arm: three
# Persisted voters on this host's disk under ~/.cache, a write every 20 ms, each member's write, step and tick
# gaps and its step-downs, elections and leader changes with their causes. The arms are interleaved and their order
# turned each round (ABBA), so both meet the host's own load alike; the load is read at each arm's start and end
# and written beside it. A load held on purpose is the caller's: LOAD_COPIES=<n> starts n looping copies of the
# before commit's whole lib suite for the run (owner-stalls.md's loaded runs used six), each its own process group,
# all stopped on any exit. Each arm's output goes to $OUT/<round>-<arm>.log; its summary lines follow at the end.
#
# usage: bench/owner_stalls_ab.sh <before> <after> <seconds> [rounds]
#   the record (persisted flush off the owner, decision of replication.md "A replica's database log"):
#     LOAD_COPIES=6 bench/owner_stalls_ab.sh db991a05d1769648d1de0390ac80b04c979d150e <the change's commit> 1800 2
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."
before="$1"; after="$2"; seconds="$3"; rounds="${4:-1}"
source bench/ab_build.sh
OUT="${OUT:-$AB_WORK/owner-stalls-$(date -u +%Y%m%dT%H%M%SZ)}"
mkdir -p "$OUT"
build() { # commit -> prints the amphora lib test binary
  local dir target
  dir=$(ab_worktree "$1")
  target="$AB_WORK/target-$(ab_tree "$1")"
  (cd "$dir" && CARGO_TARGET_DIR="$target" cargo test -p amphora --locked --lib --no-run --message-format=json 2>>"$OUT/build.log") |
    python3 -c 'import json, sys
for line in sys.stdin:
    m = json.loads(line)
    if m.get("reason") == "compiler-artifact" and m.get("executable") and m["target"]["name"] == "amphora" and m["profile"]["test"]:
        print(m["executable"])' | tail -1
}
bin_before=$(build "$before"); bin_after=$(build "$after")
[[ -x "$bin_before" && -x "$bin_after" ]] || { echo "a build failed: $OUT/build.log" >&2; exit 2; }
echo "host $(hostname) before $(git rev-parse "$before") tree $(ab_tree "$before") after $(git rev-parse "$after") tree $(ab_tree "$after") seconds $seconds rounds $rounds" | tee "$OUT/run.log"
groups=()
stop_load() { for g in ${groups[@]+"${groups[@]}"}; do kill -TERM -- "-$g" 2>/dev/null || true; done; }
trap stop_load EXIT
trap 'exit 130' INT TERM HUP
if [[ "${LOAD_COPIES:-0}" -gt 0 ]]; then
  set -m
  for _ in $(seq 1 "$LOAD_COPIES"); do
    (while :; do "$bin_before" >/dev/null 2>&1 || true; done) &
    groups+=($!)
  done
  set +m
  echo "load: $LOAD_COPIES looping copies of the before suite, process groups ${groups[*]}" | tee -a "$OUT/run.log"
fi
for round in $(seq 1 "$rounds"); do
  if ((round % 2)); then order="before after"; else order="after before"; fi
  for arm in $order; do
    if [[ $arm == before ]]; then bin=$bin_before; else bin=$bin_after; fi
    start=$(ab_load)
    set +e
    AMPHORA_STALL_SECONDS="$seconds" "$bin" --ignored --exact --nocapture \
      database::replicated::tests::production_owner_stalls_measured_on_this_host >"$OUT/$round-$arm.log" 2>&1
    code=$?
    set -e
    echo "round $round $arm exit $code load $start to $(ab_load)" | tee -a "$OUT/run.log"
  done
done
for f in "$OUT"/*-before.log "$OUT"/*-after.log; do
  echo "== $(basename "$f" .log)"
  grep -E "^member|stalls|step-downs|gap|write submit" "$f" | grep -v -E "^member [0-9]+:   term" || true
done | tee -a "$OUT/run.log"
echo "records: $OUT"
