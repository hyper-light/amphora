#!/bin/bash
# Lane P22 (serve) c1, adacachy2 (r341, needs cpus=4,mem=16g, jobs 4): the landing checks on H (perf/serve-p20-v13hold
# with main 6d8e7dcd merged): rustfmt, the lib built debug in the lane's target dir in the remote clone, QB.2's test once,
# seed_and_delta and V13 three times each, the cp_t1_13 lib tests, one replicated round, clippy, the serve binary, then each
# of the round's failures on main M (6d8e7dcd) and on H, each alone, twice, alternated, to say whether each is main's own.
set -u
H=d0c5722de0fb1085bd052ce1655856c13517af53; M=6d8e7dcd38be29214187a06bbfabdf9b054acb2a; J=4
A=$HOME/.cache/amphora-perf/serve-p7-remote
export PATH=$HOME/.cargo/bin:$PATH CARGO_HOME=$HOME/.cache/cargo-home-perf-serve-p7
export CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/serve-p7 CARGO_BUILD_JOBS=$J CARGO_INCREMENTAL=0
export CARGO_MANIFEST_DIR=$A/crates/amphora
L=$HOME/.cache/amphora-perf/serve-p22-c1/${H:0:8}; BIN=$HOME/.cache/amphora-perf-bin/serve-p22; mkdir -p "$L" "$BIN/$H" "$BIN/$M"
ld() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu-some $(sed -n 's/^some avg10=\([0-9.]*\).*/\1/p' /proc/pressure/cpu) io-some $(sed -n 's/^some avg10=\([0-9.]*\).*/\1/p' /proc/pressure/io)"; }
echo "start $(date -u +%T) jobs $J $(ld)"; df -h ~ | tail -1
git -C "$A" fetch -q origin || git -C "$A" fetch -q origin || { echo FETCH_FAILED; exit 9; }
cd "$A" && git checkout -q -f --detach "$H" || exit 9
echo "tree $(git rev-parse HEAD)"
s=$(date +%s); cargo fmt --all --check > "$L/fmt.log" 2>&1; echo "FMT_EXIT=$? $(( $(date +%s) - s ))s"; head -5 "$L/fmt.log"
s=$(date +%s); lib=$(nice -n 10 cargo test -p amphora --locked --lib --no-run 2>&1 | tee "$L/build-lib.log" | sed -n 's/.*Executable .*(\(.*\))$/\1/p' | tail -1)
[ -n "$lib" ] && [ -x "$lib" ] && cp "$lib" "$BIN/$H/lib" || { echo "BUILD_FAILED lib"; grep -E '^error|Blocking' -A12 "$L/build-lib.log" | head -40; exit 8; }
echo "built lib in $(( $(date +%s) - s ))s $(date -u +%T), $(grep -c 'Compiling amphora ' "$L/build-lib.log") amphora compiles, $(grep -c 'Compiling amphora-engine' "$L/build-lib.log") engine compiles, $(ld)"
cd "$A/crates/amphora"
one() { local o; o=$("$BIN/$H/lib" --exact "database::replicated::tests::$1" --nocapture 2>&1)
  echo "test $1: $(echo "$o" | grep -E '^test result' | tail -1 | cut -c1-90) $(date -u +%T)"
  echo "$o" | grep -E 'panicked at|stale-base|UnknownOutcome|did not move|unanswered|order to campaign|moved the clocks' | head -6; }
one qb2_the_shed_never_takes_a_hand_offs_order_to_campaign
for i in 1 2 3; do one seed_and_delta_hold_through_an_election_at_each_step; one v13_thirty_two_spread_writers_on_moving_time_are_each_answered_with_no_stale_base; done
o=$("$BIN/$H/lib" --test-threads=$J cp_t1_13 2>&1); echo "cp_t1_13 lib: $(echo "$o" | grep '^test result' | tail -1 | cut -c1-100)"; echo "$o" | grep -E 'FAILED$|panicked at' | head -10
s=$(date +%s); o=$("$BIN/$H/lib" --test-threads=$J database::replicated:: database::slot:: replication:: 2>&1); echo "$o" > "$L/round.log"
echo "round lib $(( $(date +%s) - s ))s: $(echo "$o" | grep '^test result' | tail -1 | cut -c1-110) $(ld)"; echo "$o" | grep -E '^test .* FAILED$' | head -30
cd "$A" && s=$(date +%s) && nice -n 10 cargo clippy -p amphora --all-targets --locked -- -D warnings > "$L/clippy.log" 2>&1
echo "CLIPPY_EXIT=$? $(( $(date +%s) - s ))s $(date -u +%T), $(grep -c 'Checking amphora ' "$L/clippy.log") amphora checks, engine checking $(grep -c 'Checking amphora-engine' "$L/clippy.log")"; grep -E '^(warning|error)' -A12 "$L/clippy.log" | head -30
s=$(date +%s); serve=$(nice -n 10 cargo test -p amphora --locked --test serve --no-run 2>&1 | tee "$L/build-serve.log" | sed -n 's/.*Executable .*(\(.*\))$/\1/p' | tail -1)
[ -n "$serve" ] && [ -x "$serve" ] && cp "$serve" "$BIN/$H/serve" || { echo "BUILD_FAILED serve"; grep -E '^error' -A12 "$L/build-serve.log" | head -40; }
echo "built serve in $(( $(date +%s) - s ))s $(date -u +%T)"
cd "$A/crates/amphora"
s=$(date +%s); o=$("$BIN/$H/serve" --test-threads=$J 2>&1); echo "$o" > "$L/serve.log"; echo "serve $(( $(date +%s) - s ))s: $(echo "$o" | grep '^test result' | tail -1 | cut -c1-110)"
echo "$o" | grep -E '^test (cp_t1_13.* (ok|FAILED)|.* FAILED)$' | head -20
F=$(grep -E '^test .* FAILED$' "$L/round.log" | sed -E 's/^test (.*) \.\.\. FAILED$/\1/' | sort -u | head -8)
echo "FAILED_IN_ROUND: $(echo $F | tr '\n' ' ')"; echo "comparing with main $M"
cd "$A" && git checkout -q -f --detach "$M" || exit 9
s=$(date +%s); mlib=$(nice -n 10 cargo test -p amphora --locked --lib --no-run 2>&1 | tee "$L/build-lib-main.log" | sed -n 's/.*Executable .*(\(.*\))$/\1/p' | tail -1)
[ -n "$mlib" ] && [ -x "$mlib" ] && cp "$mlib" "$BIN/$M/lib" || { echo "BUILD_FAILED main lib"; grep -E '^error' -A12 "$L/build-lib-main.log" | head -40; }
echo "built main lib in $(( $(date +%s) - s ))s $(date -u +%T)"
cd "$A/crates/amphora"
cmp() { local sha=$1 t=$2 o; o=$("$BIN/$sha/lib" --exact "$t" --nocapture 2>&1); echo "r$3 ${sha:0:8} $(basename $(echo $t | tr ':' '_')) $(echo "$o" | grep -E '^test result' | tail -1 | cut -d. -f1-2 | cut -c1-40) | $(echo "$o" | grep -A2 'panicked at' | tr '\n' ' ' | cut -c1-200)"; }
for r in 1 2; do
  if [ $((r % 2)) -eq 1 ]; then arms="$M $H"; else arms="$H $M"; fi
  for sha in $arms; do for t in $F; do cmp "$sha" "$t" "$r" & done; wait; done
done
echo "end $(date -u +%T) $(ld)"
