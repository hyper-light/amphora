#!/bin/bash
# Lane P20 (serve) m1, macOS. Arms: BRF (the branch with main and the shed fix), HOLD (BRF with V13's clock hold), RED (main with the red test), FIX
# (main with the test and the fix), M (main), BR (the branch with main, no fix). Each arm built in turn in one target
# dir, in one clone outside kiln's reach folder fetched from the synced copy. Prints each result as it comes.
set -u
[ $(df -g ~ | tail -1 | awk '{print $4}') -ge 40 ] || { echo "disk low"; df -g ~ | tail -1; exit 2; }
ROUNDS=${1:-3}
BRF=ae31667fb78711790999d3b23b9e636929b69797; HOLD=143b82cd7b85f49ade232dc3178284a776f1350d; RED=fa7c515ed44a49c7a12d2211868363f3bd00f8b7
FIX=cb6756faf43bff68844848615f871a95640f7649; M=9fbfdbaf257d452ef086c92f6528c2611f4abef2
BR=be3b72bc0047b706b93121ce15ed3baf5129fabf
src=$PWD; S=$src/bench/records/2026-11-23-adacachy2-perf-commits-v13
export PATH=$HOME/.cargo/bin:$PATH
export CARGO_HOME=$HOME/.cache/cargo-home-perf-serve-p20; mkdir -p "$CARGO_HOME"
for d in registry git bin; do [ -e "$CARGO_HOME/$d" ] || ln -s "$HOME/.cargo/$d" "$CARGO_HOME/$d"; done
T=$HOME/.cache/amphora-perf-target/serve-p17; [ -d "$T" ] || T=$HOME/.cache/amphora-perf-target/serve-p20
export CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$T CARGO_BUILD_JOBS=8 CARGO_INCREMENTAL=0 CARGO_MANIFEST_DIR=$HOME/.cache/serve-p20-ab/repo/crates/amphora
A=$HOME/.cache/serve-p20-ab/repo; L=$HOME/.cache/serve-p20-ab/m1; BIN=$HOME/.cache/amphora-perf-bin/serve-p20; mkdir -p "$L" "$BIN"
echo "host=$(hostname) target=$T $(date -u +%T) load=$(sysctl -n vm.loadavg)"; df -g ~ | tail -1
[ -d "$A/.git" ] || git init -q "$A"
git -C "$A" fetch -q "$src" '+refs/*:refs/src/*' || { echo "FETCH_FAILED"; exit 9; }
build() { # build SHA WHAT (lib, serve or bench)
  local out; cd "$A" && git checkout -q -f --detach "$1" || exit 9
  mkdir -p "$BIN/$1"; local s=$(date +%s)
  case $2 in
    lib) out=$(nice -n 10 cargo test -p amphora --locked --lib --no-run 2>&1 | tee "$L/build-lib-$1.log" | sed -n 's/.*Executable .*(\(.*\))$/\1/p' | tail -1) ;;
    serve) out=$(nice -n 10 cargo test -p amphora --locked --test serve --no-run 2>&1 | tee "$L/build-serve-$1.log" | sed -n 's/.*Executable .*(\(.*\))$/\1/p' | tail -1) ;;
    bench) (cd bench && nice -n 10 cargo build --release --locked > "$L/build-bench-$1.log" 2>&1); out=$T/release/amphora-bench ;;
  esac
  [ -n "$out" ] && [ -x "$out" ] && cp "$out" "$BIN/$1/$2" || { echo "BUILD_FAILED $2 ${1:0:8}"; grep -E '^error' -A14 "$L"/build-*-"$1".log | head -60; return 1; }
  echo "built $2 ${1:0:8} in $(( $(date +%s) - s ))s $(date -u +%T)"
  cd "$A/crates/amphora"
}
one() { # one SHA NAME: one test alone, its verdict and its panic
  local o; o=$("$BIN/$1/lib" --exact "database::replicated::tests::$2" --nocapture 2>&1)
  echo "test ${1:0:8} $2: $(echo "$o" | grep -E '^test result' | tail -1) $(date -u +%T)"
  echo "$o" | grep -E 'panicked at|stale-base|UnknownOutcome|did not move|unanswered|order to campaign|moved the clocks' | head -6
}
QB=qb2_the_shed_never_takes_a_hand_offs_order_to_campaign; SD=seed_and_delta_hold_through_an_election_at_each_step
V13=v13_thirty_two_spread_writers_on_moving_time_are_each_answered_with_no_stale_base
build $HOLD lib && { one $HOLD $QB; for i in $(seq 1 3); do one $HOLD $SD; one $HOLD $V13; done
  "$BIN/$HOLD/lib" --test-threads=4 cp_t1_13 2>&1 | grep -E 'FAILED$|^test .* ok$|^test result|panicked at' | head -20; }
cd "$A" && git checkout -q -f --detach $HOLD && s=$(date +%s) && nice -n 10 cargo clippy -p amphora --all-targets --locked -- -D warnings > "$L/clippy.log" 2>&1; echo "CLIPPY_EXIT=$? ${HOLD:0:8} $(( $(date +%s) - s ))s $(date -u +%T)"; grep -E '^(warning|error)' -A12 "$L/clippy.log" | head -40; cd "$A/crates/amphora"
build $HOLD bench && build $M bench && for r in $(seq 1 "$ROUNDS"); do
  if [ $((r % 2)) -eq 1 ]; then arms="$HOLD $M"; else arms="$M $HOLD"; fi
  for sha in $arms; do for cls in persisted replicated; do
    echo "cell r$r ${sha:0:8} $cls $(date -u +%T) load=$(sysctl -n vm.loadavg) $(nice -n 10 python3 -I "$S/cell13.py" "$BIN/$sha/bench" 29620 "$L/cells/${sha:0:8}-$cls-32-r$r" 32 spread "$cls")"
  done; done
done
build $RED lib && { one $RED $QB; one $RED $SD; one $RED $V13; }
build $HOLD serve && { s=$(date +%s); o=$("$BIN/$HOLD/serve" --test-threads=4 2>&1); echo "round ${HOLD:0:8} serve $(( $(date +%s) - s ))s: $(echo "$o" | grep '^test result' | tail -1)"
  echo "$o" | grep -E '^test (cp_t1_13.* (ok|FAILED)|.* FAILED)$' | head -20; }
s=$(date +%s); o=$("$BIN/$HOLD/lib" --test-threads=8 database::replicated:: database::slot:: replication:: 2>&1); echo "round ${HOLD:0:8} lib $(( $(date +%s) - s ))s: $(echo "$o" | grep '^test result' | tail -1)"
echo "$o" | grep -E '^test .* FAILED$' | head -30
build $FIX lib && one $FIX $QB
build $BRF lib && one $BRF $V13
echo "end $(date -u +%T) load=$(sysctl -n vm.loadavg)"
