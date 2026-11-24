# Lane B26's evidence: the base made before lane B22, the red arm, the final tree (bash). env: FINAL, RED, JOBS, STEPS
set -uo pipefail
export CARGO_HOME=$HOME/.cache/cargo-home-perf-pagepath CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/pagepath CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${JOBS:-6}
B=$HOME/.cache/amphora-perf/pagepath-b26; export B26_OLD=$B/old-base
cd $HOME/.cache/amphora-perf/pagepath-remote || exit 3
git fetch -q origin || exit 3
git checkout -q -f . && git clean -q -fd crates
load() { cut -d' ' -f1-3 /proc/loadavg; }
say() { echo "== $* load $(load) $(date -u +%H:%M:%SZ)"; }
t() { nice -n 10 cargo test -p amphora --locked "$@"; }
NEW="a_base_that a_base_whose a_restore_that_fails a_killed_restore a_restore_a_full a_cut_one a_failed_check d45_a_check_clears d45_a_base_begun_after"
want=${STEPS:-old red green}
if [[ " $want " == *" old "* ]]; then
  git checkout -q --detach ba250493 || exit 3
  python3 $B/old_write.py || exit 4
  rm -rf "${B:?}/old-base"; mkdir -p "$B26_OLD"
  say "the tree before lane B22's fix, $(git rev-parse --short HEAD), writes"
  t --test backup -- --test-threads=1 d45_a_base_begun_while > $B/old-write.log 2>&1; echo "exit $?"; grep -E "^error|^test |test result|lost" $B/old-write.log | cut -c1-260 | head -8
  ls $B26_OLD; tail -1 $B26_OLD/history.txt | cut -c1-60
  git checkout -q -f .
fi
if [[ " $want " == *" red "* ]]; then
  git checkout -q --detach ${RED:?} || exit 3
  python3 $B/old_read.py
  say "red arm $(git rev-parse --short HEAD): main's code, the lane's tests, the base made before"
  t --test backup --no-run > $B/red-build.log 2>&1; r=$?; echo "build exit $r"; [ $r = 0 ] || { grep -E '^error' -A12 $B/red-build.log | head -60; exit 5; }
  t --test backup -- --test-threads=2 --nocapture b26_old_base > $B/red-old.log 2>&1; echo "the base made before: exit $?"; grep -E "^B26|test result|panicked" -A1 $B/red-old.log | cut -c1-300 | head -20
  t --test backup -- --test-threads=6 $NEW > $B/red-t.log 2>&1; echo "tests/backup.rs exit $?"; grep -E "^test .*(FAILED|ok$)|test result" $B/red-t.log
  grep -E "panicked|restored with no error|opens,|left:|no base begun|opened:" -A2 $B/red-t.log | cut -c1-420 | head -60
  git checkout -q -f .
fi
if [[ " $want " == *" green "* ]]; then
  git checkout -q --detach ${FINAL:?} || exit 3
  python3 $B/old_read.py
  say "final $(git rev-parse --short HEAD): the base made before"
  t --test backup -- --test-threads=2 --nocapture b26_old_base > $B/green-old.log 2>&1; echo "the base made before: exit $?"; grep -E "^error|^B26|test result|panicked" $B/green-old.log | cut -c1-300 | head -20
  git checkout -q -f .
fi
say done
