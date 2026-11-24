#!/usr/bin/env bash
# Lane A27: the loom tests on line.rs as it is, then on broken copies of it, each held to the tests that must turn red.
set -uo pipefail
SHA=${1:?}; OUT=$HOME/.cache/amphora-perf/cross-a27-out; mkdir -p "$OUT"
export CARGO_HOME=$HOME/.cache/cargo-home-perf-cross PATH=$HOME/.cargo/bin:$PATH CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/cross CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0
R=$HOME/.cache/amphora-perf/cross-remote; W=$HOME/.cache/amphora-perf/cross-a27-wt
cd "$W" && git checkout -q -f --detach "$SHA" && git log --oneline -1
L=crates/amphora/src/database/replicated/line.rs
build() { RUSTFLAGS="--cfg loom" timeout 300 nice -n 10 cargo test -p amphora-line-loom --release --locked --lib --no-run > "$OUT/loom-build-$1.log" 2>&1 || { echo "$1 BUILD FAILED" >&2; grep -E '^error' -A 10 "$OUT/loom-build-$1.log" | head -20 >&2; return 1; }; sed -n 's/.*Executable unittests src\/lib.rs (\(.*\))/\1/p' "$OUT/loom-build-$1.log"; }
run1() { local bin=$1 label=$2 t=$3 l s rc; l="$OUT/loom-$label-$(echo "$t" | tr : _).log"; s=$SECONDS; timeout ${TT:-150} nice -n 10 "$bin" --exact "$t" --test-threads=1 --nocapture > "$l" 2>&1; rc=$?; echo "  $label ${t#tests::t_qb_1_19_} rc=$rc $((SECONDS-s))s $(grep -o '[0-9]* interleavings' "$l" | tail -1) $([ $rc = 0 ] || grep -hoE 'deadlock|two hold one place|admitted uncharged|admitted without its charge|left: .*|right: .*' "$l" | sort -u | head -3 | tr '\n' ' ')"; }
bin=$(build real) || exit 1; cp "$bin" "$HOME/.cache/amphora-perf-bin/cross/a27-loom-$SHA"; bin=$HOME/.cache/amphora-perf-bin/cross/a27-loom-$SHA
for t in $("$bin" --list 2>/dev/null | sed -n 's/: test$//p'); do run1 "$bin" real "$t"; done
mutate() { local label=$1 tests=$2; shift 2; git checkout -q -- $L; python3 - "$L" "$@" <<'PY'
import sys
p,a,b=sys.argv[1:4]
s=open(p).read()
assert s.count(a)>=1,(a,"not found")
open(p,'w').write(s.replace(a,b,1))
PY
[ $? = 0 ] || { echo "$label: mutation did not apply"; return; }
b=$(build "$label") || return; for t in $tests; do TT=60 run1 "$b" "$label" "tests::t_qb_1_19_$t"; done; }
mutate M1-no-fence-in-join 'a_place_given_back_reaches_the_call_that_waits_for_it' '        fence(Ordering::SeqCst);
        Ok(Turn {' '        Ok(Turn {'
mutate M2-next-not-rung 'two_places_given_back_at_once_reach_both_calls_in_line a_deadline_beside_a_give_back_leaves_the_next_call_its_room' '            self.line.waiting.fetch_sub(1, Ordering::SeqCst);
            if let Some(first) = turns.line.values().next() {
                first.ring();
            }' '            self.line.waiting.fetch_sub(1, Ordering::SeqCst);'
mutate M3-room-came-relaxed 'a_place_given_back_reaches_the_call_that_waits_for_it' 'if self.waiting.load(Ordering::SeqCst) == 0 {' 'if self.waiting.load(Ordering::Relaxed) == 0 {'
mutate M4-newcomers-barge 'a_call_that_comes_does_not_go_before_one_in_line' 'if line.waiting.load(Ordering::Acquire) == 0
            && let Ok(place)' 'if let Ok(place)'
mutate M5-end-rings-none 'the_owners_end_wakes_every_call_in_line' '            for bell in turns.line.values() {
                bell.ring();
            }' '            let _ = reason;'
mutate M6-any-in-line-takes 'the_first_in_line_is_the_first_admitted_and_no_place_is_taken_twice a_call_that_comes_does_not_go_before_one_in_line' 'if turn.first()?
                && past.is_none()' 'if turn.first().is_ok()
                && past.is_none()'
mutate M8-bytes-back-unrung 'bytes_given_back_reach_the_call_that_waits_for_them' '            self.room.charged.fetch_sub(bytes, Ordering::AcqRel);
        }
        self.room.line.room_came();' '            self.room.charged.fetch_sub(bytes, Ordering::AcqRel);
        }'
mutate M9-admitted-uncharged 'a_granted_place_whose_budget_room_went_waits_for_it_again' 'Err(Refusal::TooLarge { .. }) => {
                    drop(place);
                    drop(turn);
                }' 'Err(Refusal::TooLarge { .. }) => return Ok((place, turn)),'
mutate M10-out-of-line-before-it-sends 'calls_are_sent_in_the_order_they_stood_in_line' 'return Ok((place, Some(turn)));' 'drop(turn); return Ok((place, None));'
git checkout -q -- $L; echo "[$(date -u +%H:%M:%SZ)] end"
