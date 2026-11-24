#!/usr/bin/env bash
# Lane A28: the budget's wait and the line's models on the files as they are, then on broken copies of them, each held to the models that must turn red.
set -uo pipefail
SHA=${1:?}; OUT=$HOME/.cache/amphora-perf/cross-a28-out; mkdir -p "$OUT"; T=${JOBS:-3}
export CARGO_HOME=$HOME/.cache/cargo-home-perf-cross PATH=$HOME/.cargo/bin:$PATH CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/cross CARGO_BUILD_JOBS=$T CARGO_INCREMENTAL=0
R=$HOME/.cache/amphora-perf/cross-remote; W=${W:-$HOME/.cache/amphora-perf/cross-a28-wt-mut}
git -C "$R" fetch -q origin perf/cross-a28; [ -d "$W" ] || git -C "$R" worktree add -q --detach "$W" "$SHA"; cd "$W" && git checkout -q -f --detach "$SHA" && git log --oneline -1
WAIT=crates/amphora/src/memory/wait.rs; L=crates/amphora/src/database/replicated/line.rs
build() { RUSTFLAGS="--cfg loom" timeout 300 nice -n 10 cargo test -p amphora-line-loom --release --locked --lib --no-run > "$OUT/mut-build-$1.log" 2>&1 || { echo "$1 BUILD FAILED" >&2; grep -E '^error' -A 10 "$OUT/mut-build-$1.log" | head -20 >&2; return 1; }; sed -n 's/.*Executable unittests src\/lib.rs (\(.*\))/\1/p' "$OUT/mut-build-$1.log"; }
run1() { local bin=$1 label=$2 t=$3 l s rc; l="$OUT/mut-$label-$(echo "$t" | tr : _).log"; s=$SECONDS; timeout ${TT:-100} nice -n 10 "$bin" --exact "$t" --test-threads=1 --nocapture > "$l" 2>&1; rc=$?; echo "  $label ${t#tests::} rc=$rc $((SECONDS-s))s $(grep -o '[0-9]* interleavings' "$l" | tail -1) $([ $rc = 0 ] || grep -hoE 'deadlock|left: .*|right: .*|assertion[^\n]{0,60}' "$l" | sort -u | head -3 | tr '\n' ' ')"; }
bin=$(build real) || exit 1; cp "$bin" "$HOME/.cache/amphora-perf-bin/cross/a28-mut-real"; bin=$HOME/.cache/amphora-perf-bin/cross/a28-mut-real
[ -n "${ONLY:-}" ] || for t in $("$bin" --list 2>/dev/null | sed -n 's/: test$//p' | grep -E 't_mb_1_5|owners_end'); do run1 "$bin" real "$t"; done
mutate() { local label=$1 file=$2 tests=$3; shift 3; [ -z "${ONLY:-}" ] || { local o; for o in $ONLY; do [ "${label%%-*}" = "$o" ] && break; done; [ "${label%%-*}" = "$o" ] || return 0; }; git checkout -q -- crates; python3 - "$file" "$@" <<'PY'
import sys
p,a,b=sys.argv[1:4]
s=open(p).read()
assert s.count(a)>=1,(a,"not found")
open(p,'w').write(s.replace(a,b,1))
PY
[ $? = 0 ] || { echo "$label: mutation did not apply"; return; }
b=$(build "$label") || return; for t in $tests; do run1 "$b" "$label" "tests::$t"; done; }
E=t_mb_1_5_an_owners_end_reaches_a_holder_that_waits; G=t_mb_1_5_bytes_given_back_reach_a_holder_that_waits; B=t_mb_1_5_bytes_given_back_beside_the_end_are_charged_or_not_never_held_by_a_refusal; O=t_mb_1_5_an_owners_end_ends_no_other_owners_wait; Q=t_qb_1_19_the_owners_end_reaches_a_call_that_waits_for_the_budget; Y=t_qb_1_19_the_owners_end_wakes_every_call_in_line
mutate W1-the-reason-is-not-asked-mains-rule $WAIT "$E $Q" '            if let Some(reason) = owner.reason() {
                break Err(reason);
            }' '            let _ = owner.reason();'
mutate W2-the-end-tells-no-one $WAIT "$E $Q" '        for waiters in watched {
            waiters.look_again();
        }' '        drop(watched);'
mutate W3-told-before-the-reason-is-set $WAIT "$E $B" '        let mut watched = {
            let mut state = self.locked();
            state.reason.get_or_insert_with(|| reason.clone());
            std::mem::take(&mut state.watched)
        };
        // Told once for a budget, however many wait on it.
        watched.dedup_by(|a, b| Arc::ptr_eq(a, b));
        for waiters in watched {
            waiters.look_again();
        }' '        let mut watched = std::mem::take(&mut self.locked().watched);
        watched.dedup_by(|a, b| Arc::ptr_eq(a, b));
        for waiters in watched {
            waiters.look_again();
        }
        self.locked().reason.get_or_insert_with(|| reason.clone());'
mutate W4-no-fence $WAIT "$G $B" '        fence(Ordering::SeqCst);' ''
mutate W5-holders-read-relaxed $WAIT "$G $B" 'if self.holders.load(Ordering::SeqCst) > 0 {' 'if self.holders.load(Ordering::Relaxed) > 0 {'
mutate W6-the-watch-never-taken-off $WAIT "$O $B" '            state.watched.swap_remove(at);' '            let _ = at;'
mutate W7-one-told-not-all $WAIT "$O" '        self.telling.notify_all();' '        self.telling.notify_one();'
mutate W8-the-line-tells-no-ending $L "$Q $Y" '        self.ending.end(reason);' '        let _ = reason;'
git checkout -q -- crates; echo "[$(date -u +%H:%M:%SZ)] end"
