#!/usr/bin/env bash
# Lane A27: the fix's lib test binary and its touched tests, the order test a hundred times a side, then the red arms.
set -uo pipefail
SHA=${1:?}; OUT=$HOME/.cache/amphora-perf/cross-a27-out; T=${JOBS:-6}
export CARGO_HOME=$HOME/.cache/cargo-home-perf-cross PATH=$HOME/.cargo/bin:$PATH CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/cross CARGO_BUILD_JOBS=$T CARGO_INCREMENTAL=0
R=$HOME/.cache/amphora-perf/cross-remote; W=$HOME/.cache/amphora-perf/cross-a27-wt2; B=$HOME/.cache/amphora-perf-bin/cross
git -C "$R" fetch -q origin main perf/cross-a27; cd "$W" && git checkout -q -f --detach "$SHA" && git log --oneline -1
stamp() { echo "[$(date -u +%H:%M:%SZ)] $* load=$(cut -d' ' -f1-3 /proc/loadavg)"; }
build() { local s=$(date +%s); timeout 900 nice -n 10 cargo test -p amphora --lib --locked --no-run > "$OUT/build-$1.log" 2>&1 || { stamp "$1 build FAILED"; grep -E '^error' -A 12 "$OUT/build-$1.log" | head -50; return 1; }; local bin=$(sed -n 's/.*Executable unittests src\/lib.rs (\(.*\))/\1/p' "$OUT/build-$1.log"); cp "$bin" "$B/a27-tests-$1"; stamp "$1 built $(( $(date +%s) - s ))s"; }
tests() { local label=$1; shift; timeout 900 nice -n 10 "$B/a27-tests-$label" --test-threads=$T --nocapture "$@" > "$OUT/t-$label.log" 2>&1; rc=$?; stamp "$label rc=$rc: $(grep 'test result' "$OUT/t-$label.log" | tail -1)"; grep -E '^test .* (FAILED|failed)$' "$OUT/t-$label.log" | head -20; grep -E 'panicked at' -A 3 "$OUT/t-$label.log" | cut -c1-300 | head -${LINES_MAX:-24}; grep -E 'full at a near' "$OUT/t-$label.log"; }
order() { local bin=$1 label=$2 n=$3 bad=0; cd "$W/crates/amphora"; for i in $(seq 1 $n); do timeout 100 nice -n 10 "$bin" --test-threads=8 --exact database::replicated::tests::a26_a_call_past_the_queues_count_waits_in_line_and_a_wire_record_does_not > "$OUT/order-$label.log" 2>&1 || bad=$((bad+1)); done; stamp "order test, $label: $bad of $n failed"; grep -E 'left:|right:' "$OUT/order-$label.log" | head -2; }
(cd crates/amphora && build $SHA) || exit 1
(cd crates/amphora && tests $SHA a27_ a26_ a21_ a11_ unknown_leader_application_pressure t_qb_1)
# The order test under load of its own: eight side by side, each twenty-five times, the main tree's binary then the fix's.
M=$B/a26-tests-04eb931a9189d603671baa0e42f47c75e8adedcb
many() { local bin=$1 label=$2; cd "$W/crates/amphora"; rm -f "$OUT"/many-$label-*.txt; for k in $(seq 1 8); do ( bad=0; for i in $(seq 1 25); do timeout 100 nice -n 10 "$bin" --exact database::replicated::tests::a26_a_call_past_the_queues_count_waits_in_line_and_a_wire_record_does_not >/dev/null 2>&1 || bad=$((bad+1)); done; echo $bad > "$OUT/many-$label-$k.txt" ) & done; wait; stamp "order test, $label, eight side by side, 25 each: $(paste -sd+ "$OUT"/many-$label-*.txt | bc 2>/dev/null || cat "$OUT"/many-$label-*.txt | awk '{s+=$1} END{print s}') of 200 failed"; cd "$W"; }
many $M main; many $B/a27-tests-$SHA fix
mutate() { local label=$1 file=$2; shift 2; git checkout -q -- crates; python3 - "$file" "$@" <<'PY'
import sys
p,a,b=sys.argv[1:4]
s=open(p).read()
assert s.count(a)==1,(s.count(a),a)
open(p,'w').write(s.replace(a,b,1))
PY
[ $? = 0 ] || { echo "$label: mutation did not apply"; return 1; }; (cd crates/amphora && build "$label"); }
Q=crates/amphora/src/database/replicated.rs; L=crates/amphora/src/database/replicated/line.rs
mutate red1-main-alone $Q '        .saturating_sub(wire.min(message))' '        .saturating_sub(u64::try_from(RECORD_BYTES).unwrap_or(u64::MAX))' && (cd crates/amphora && tests red1-main-alone a27_no_call a27_a_group a26_the_record)
mutate red2-3bef6837-alone $Q '    let alone = queued
        .saturating_sub(wire.min(message))
        .saturating_sub(envelope);' '    let alone = { let _ = message; queued };' && (cd crates/amphora && tests red2-3bef6837-alone a27_no_call a27_a_group)
mutate red3-admitted-uncharged $L '                Err(Refusal::TooLarge { .. }) => {
                    drop(place);
                    drop(turn);
                }' '                Err(Refusal::TooLarge { .. }) => return Ok((place, turn)),' && (cd crates/amphora && tests red3-admitted-uncharged a27_a_place)
mutate red4-out-of-line-before-it-sends $L 'return Ok((place, Some(turn)));' 'drop(turn); return Ok((place, None));' && many $B/a27-tests-red4-out-of-line-before-it-sends red4
git checkout -q -- crates; stamp end
