# Lane B25: each part of the fix taken out, the tests that answer for it (bash). env: FINAL, MUTS, JOBS
set -uo pipefail
export CARGO_HOME=$HOME/.cache/cargo-home-perf-pagepath CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/pagepath CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${JOBS:-6}
B=$HOME/.cache/amphora-perf/pagepath-b25; cd $HOME/.cache/amphora-perf/pagepath-remote || exit 3
git fetch -q origin || exit 3
NEW="a_restore_of_ a_short_restore a_base_begun a_shipper_opened d45_a_shipper_opened_after d45_a_generation_ended d45_a_new_database d45_a_fence_told d45_open_discards d45_a_base d45_a_send"
for m in ${MUTS:?}; do
  git checkout -q -f . && git checkout -q --detach ${FINAL:?} || exit 3
  echo "== $(python3 $B/mutants.py $m) load $(cut -d' ' -f1-3 /proc/loadavg) $(date -u +%H:%M:%SZ)"
  nice -n 10 cargo test -p amphora --locked --lib --test backup --no-run > $B/mut-$m-build.log 2>&1; r=$?
  if [ $r != 0 ]; then echo "build exit $r"; grep -E '^error' -A8 $B/mut-$m-build.log | head -20; continue; fi
  nice -n 10 cargo test -p amphora --locked --test backup -- --test-threads=4 $NEW > $B/mut-$m-t.log 2>&1
  nice -n 10 cargo test -p amphora --locked --lib -- --test-threads=4 backup:: > $B/mut-$m-l.log 2>&1
  grep -hE "^test .*FAILED|^test result" $B/mut-$m-t.log $B/mut-$m-l.log
done
git checkout -q -f .
echo "done $(date -u +%H:%M:%SZ)"
