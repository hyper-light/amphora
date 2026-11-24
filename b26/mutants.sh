# Lane B26's mutants, each part of the fix taken out (bash). env: SHA, JOBS, ONLY
set -uo pipefail
export CARGO_HOME=$HOME/.cache/cargo-home-perf-pagepath CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/pagepath CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${JOBS:-6}
B=$HOME/.cache/amphora-perf/pagepath-b26; O=$B/mutants; mkdir -p "$O"
cd $HOME/.cache/amphora-perf/pagepath-remote || exit 3
git fetch -q origin || exit 3
git checkout -q -f . && git clean -q -fd crates && git checkout -q --detach "${SHA:?}" || exit 3
echo "mutants of $(git rev-parse HEAD) load $(cut -d' ' -f1-3 /proc/loadavg) $(date -u +%H:%M:%SZ)"
T1="a_base_whose a_restore_that_fails a_killed_restore a_restore_a_full a_cut_one a_failed_check d45_a_check_clears d45_a_base_begun a_base_that a_short_restore"
for m in ${ONLY:-$(python3 $B/mutants.py list)}; do
  git checkout -q -f . ; python3 $B/mutants.py "$m" || { echo "$m: not applied"; continue; }
  nice -n 10 cargo test -p amphora --locked --lib --test backup --no-run > "$O/$m.build" 2>&1 || { echo "$m: build failed"; grep -E '^error' -A8 "$O/$m.build" | head -20; continue; }
  nice -n 10 cargo test -p amphora --locked --test backup -- --test-threads=6 $T1 > "$O/$m.t" 2>&1
  nice -n 10 cargo test -p amphora --locked --lib -- --test-threads=4 backup:: > "$O/$m.l" 2>&1
  echo "$m $(date -u +%H:%M:%SZ): red: $(grep -hE '^test .*FAILED' "$O/$m.t" "$O/$m.l" | sed 's/^test //; s/ \.\.\. FAILED//' | tr '\n' ' ')"
  grep -hE "panicked at" -A2 "$O/$m.t" "$O/$m.l" | grep -vE "panicked at|^--|note:" | cut -c1-260 | head -6
done
git checkout -q -f .
echo "done load $(cut -d' ' -f1-3 /proc/loadavg) $(date -u +%H:%M:%SZ)"
