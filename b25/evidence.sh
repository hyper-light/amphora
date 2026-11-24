# Lane B25's evidence: the pre-B24 archive, the red arm, the mutants (bash). env: FINAL, RED, JOBS
set -uo pipefail
export CARGO_HOME=$HOME/.cache/cargo-home-perf-pagepath CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/pagepath CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${JOBS:-6}
B=$HOME/.cache/amphora-perf/pagepath-b25; export B25_OLD_ARCHIVE=$B/old-archives
cd $HOME/.cache/amphora-perf/pagepath-remote || exit 3
git fetch -q origin || exit 3
git checkout -q -f . && git clean -q -fd crates
load() { cut -d' ' -f1-3 /proc/loadavg; }
say() { echo "== $* load $(load) $(date -u +%H:%M:%SZ)"; }
t() { nice -n 10 cargo test -p amphora --locked "$@"; }
want=${STEPS:-old red mutants}
if [[ " $want " == *" old "* ]]; then
  git checkout -q --detach 46e2480f || exit 3
  python3 $B/old_write.py || exit 4
  rm -rf "${B:?}/old-archives"; mkdir -p "$B25_OLD_ARCHIVE"
  say "old tree $(git rev-parse --short HEAD) writes"
  t --test backup -- --test-threads=1 d45_a_send > $B/old-write.log 2>&1; echo "exit $?"; grep -E "^error|^test |test result|lost" $B/old-write.log | cut -c1-300 | head -12
  ls $B25_OLD_ARCHIVE; du -sh $B25_OLD_ARCHIVE
  git checkout -q -f .
fi
if [[ " $want " == *" red "* ]]; then
  git checkout -q --detach ${RED:?} || exit 3
  python3 $B/old_read.py
  say "red arm $(git rev-parse --short HEAD): main's code, every scenario test and the old archives"
  t --lib --test backup --no-run > $B/red-build.log 2>&1; r=$?; echo "build exit $r"; [ $r = 0 ] || { grep -E '^error' -A12 $B/red-build.log | head -60; exit 5; }
  t --test backup -- --test-threads=4 --nocapture b25_old_archives > $B/red-old.log 2>&1; echo "old archives exit $?"; grep -E "^r[12]:|test result|panicked" $B/red-old.log | cut -c1-400
  t --test backup -- --test-threads=4 a_restore_of_ a_short_restore a_base_begun d45_a_shipper_opened_after d45_a_generation_ended d45_a_new_database > $B/red-t.log 2>&1; echo "tests/backup.rs exit $?"; grep -E "^test .*(FAILED|ok$)|test result" $B/red-t.log
  grep -E "restored with no error|its end Ok|refused otherwise|at its path" $B/red-t.log | cut -c1-420
  t --lib -- --test-threads=4 a_base_begun a_shipper_opened_on d45_a_generation_ended_by_force_is_whole_once_shipped > $B/red-l.log 2>&1; echo "lib exit $?"; grep -E "^test .*(FAILED|ok$)|test result" $B/red-l.log
  nice -n 10 cargo test -p amphora-engine --lib --locked -- d45_open_discards > $B/red-e.log 2>&1; echo "engine exit $?"; grep -E "^test .*(FAILED|ok$)|test result|panicked|left:|right:" -A1 $B/red-e.log | head
  git checkout -q -f .
fi
if [[ " $want " == *" green "* ]]; then
  git checkout -q --detach ${FINAL:?} || exit 3
  python3 $B/old_read.py
  say "green $(git rev-parse --short HEAD): the old archives"
  t --test backup -- --test-threads=4 --nocapture b25_old_archives > $B/green-old.log 2>&1; echo "old archives exit $?"; grep -E "^error|^r[12]:|test result|panicked" $B/green-old.log | cut -c1-400
  git checkout -q -f .
fi
say done
