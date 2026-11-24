# Lane B26's mutants, several parts of the fix taken out at once where their tests are apart (bash). env: SHA, JOBS
set -uo pipefail
export CARGO_HOME=$HOME/.cache/cargo-home-perf-pagepath CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/pagepath CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${JOBS:-6}
B=$HOME/.cache/amphora-perf/pagepath-b26; O=$B/mutants; mkdir -p "$O"
cd $HOME/.cache/amphora-perf/pagepath-remote || exit 3
git fetch -q origin || exit 3
git checkout -q -f . && git clean -q -fd crates && git checkout -q --detach "${SHA:?}" || exit 3
echo "mutants of $(git rev-parse HEAD) load $(cut -d' ' -f1-3 /proc/loadavg) $(date -u +%H:%M:%SZ)"
T1="a_base_whose a_restore_that_fails a_killed_restore a_restore_a_full a_cut_one a_failed_check d45_a_check_clears d45_a_base_begun a_base_that a_short_restore"
A="m1-manifest-not-compared m6-its-path-not-refused m9-the-ask-wakes-nothing m11-each-group-put-at-its-path-as-it-is-made m12-a-killed-checks-files-kept m13-no-bound-on-the-asks"
Bb="m2-own-generation-not-compared m7-its-name-not-refused m8-a-failed-check-asks-nothing m4-a-failed-restores-files-kept m14-the-base-for-it-keeps-chunks"
Cc="m3-begun-before-the-segment-is-in-the-file m5-made-at-its-own-path m10-the-log-not-shipped-again"
for set in "$A" "$Bb" "$Cc"; do
  git checkout -q -f . ; ok=1; for m in $set; do python3 $B/mutants.py "$m" || { echo "$m: not applied"; ok=0; }; done; [ $ok = 1 ] || continue
  n=$(echo "$set" | cut -d- -f1)
  nice -n 10 cargo test -p amphora --locked --lib --test backup --no-run > "$O/set-$n.build" 2>&1 || { echo "set of $set: build failed"; grep -E '^error' -A8 "$O/set-$n.build" | head -20; continue; }
  ( timeout 100 nice -n 10 cargo test -p amphora --locked --lib -- --test-threads=2 backup:: > "$O/set-$n.l" 2>&1 ) &
  timeout 100 nice -n 10 cargo test -p amphora --locked --test backup -- --test-threads=6 $T1 > "$O/set-$n.t" 2>&1; wait
  echo "taken out: $set $(date -u +%H:%M:%SZ): red: $(grep -hE '^test .*FAILED' "$O/set-$n.t" "$O/set-$n.l" | sed 's/^test //; s/ \.\.\. FAILED//' | tr '\n' ' ')"
  grep -hE "^thread '.*panicked at" -A3 "$O/set-$n.t" "$O/set-$n.l" | grep -vE "^--|note:" | cut -c1-260 | head -40
done
git checkout -q -f .
echo "done load $(cut -d' ' -f1-3 /proc/loadavg) $(date -u +%H:%M:%SZ)"
