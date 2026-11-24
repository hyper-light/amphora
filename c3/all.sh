#!/usr/bin/env bash
# Lane C3: both arms in one run. main = this tree; fix = a copy of it with c3/candidate.patch. Each arm's
# t_pl_8_13 on six fresh calibrations, plans weighed, forced shapes, the automatic index's timing, every
# setting's answers judged by sqlite3, and the sweep, all at once (11 processes an arm). usage: JOBS=16 bash c3/all.sh
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd); cd "$ROOT" || exit 2
. c3/env.sh
L=$HOME/.cache/amphora-perf/planner-c3-logs/ab-$(hostname -s); mkdir -p "$L"
build() {
  ( cd "$1" && nice -n 10 cargo test -p amphora-engine --lib --locked --no-run >"$L/build-$2.log" 2>&1 ); local rc=$?
  grep -cE 'Compiling amphora-engine ' "$L/build-$2.log" | sed "s/^/$2: compiled amphora-engine: /"; echo "build $2 rc=$rc $(date -u +%H:%M:%SZ)"
  [ $rc = 0 ] || { grep -E '^error' -A14 "$L/build-$2.log" | head -60; return 4; }
  local bin; bin=$(sed -n 's/^ *Executable .*(\(.*\))$/\1/p' "$L/build-$2.log" | tail -1)
  case "$bin" in /*) ;; *) bin="$1/$bin";; esac
  cp "$bin" "$BINS/ab-$2"
}
build "$ROOT" main || exit 4
F=$HOME/.cache/amphora-perf/planner-c3-fixtree; rm -rf "${F:?}"; cp -R "$ROOT" "$F" && ( cd "$F" && git apply c3/candidate.patch && git diff --stat ) || exit 5
build "$F" fix || exit 4
cd "$ROOT/crates/engine" || exit 2
xdg() { X=$HOME/.cache/amphora-perf/planner-c3-xdg/ab-$1; rm -rf "${X:?}"; mkdir -p "$X"; echo "$X"; }
for arm in main fix; do
  P="$BINS/ab-$arm"; pids=""
  echo "=== $arm $(date -u +%H:%M:%SZ) $(uptime | sed 's/.*load/load/')"
  for i in 1 2 3 4 5 6; do
    ( XDG_CACHE_HOME=$(xdg $arm-t$i) nice -n 10 "$P" t_pl_8_13_a_left_join_inside --nocapture --test-threads=1 >"$L/$arm-t$i.log" 2>&1 ) & pids="$pids $!"
  done
  ( XDG_CACHE_HOME=$(xdg $arm-w) RUST_LOG=amphora::join_search=debug nice -n 10 "$P" c3_probe_weighed --nocapture --test-threads=1 >"$L/$arm-weighed.log" 2>&1 ) & pids="$pids $!"
  ( XDG_CACHE_HOME=$(xdg $arm-f) nice -n 10 "$P" c3_probe_forced --nocapture --test-threads=1 >"$L/$arm-forced.log" 2>&1 ) & pids="$pids $!"
  ( XDG_CACHE_HOME=$(xdg $arm-i) nice -n 10 "$P" c3_probe_autoindex --nocapture --test-threads=1 >"$L/$arm-autoindex.log" 2>&1 ) & pids="$pids $!"
  ( C3_DIR=$L/$arm-answers XDG_CACHE_HOME=$(xdg $arm-a) nice -n 10 "$P" c3_probe_answers --nocapture --test-threads=1 >"$L/$arm-answers.log" 2>&1; python3 "$ROOT/c3/judge.py" "$L/$arm-answers" >"$L/$arm-judge.log" 2>&1 ) & pids="$pids $!"
  ( XDG_CACHE_HOME=$(xdg $arm-s) nice -n 10 "$P" c3_sweep --nocapture --test-threads=1 >"$L/$arm-sweep.log" 2>&1 ) & pids="$pids $!"
  wait $pids
  for i in 1 2 3 4 5 6; do echo "--- $arm t_pl_8_13 round $i"; grep -E '^bushy=' "$L/$arm-t$i.log" | cut -c1-24; grep -E 'panicked|not planned|no LEFT|test result' "$L/$arm-t$i.log" | cut -c1-330; done
  echo "--- $arm weighed"; grep -E 'C3W' "$L/$arm-weighed.log" | cut -c1-520
  echo "--- $arm forced"; grep -E 'C3F|panicked|test result' "$L/$arm-forced.log" | cut -c1-900
  echo "--- $arm automatic index"; grep -E 'C3I|panicked|test result' "$L/$arm-autoindex.log" | cut -c1-700
  echo "--- $arm answers against SQLite"; cut -c1-400 "$L/$arm-judge.log" | head -70
  echo "--- $arm sweep"; grep -E 'C3S|panicked|test result' "$L/$arm-sweep.log" | grep -v 'WRONG|' | cut -c1-230
  grep -E 'C3S\|WRONG' "$L/$arm-sweep.log" | cut -c1-900 | head -${WRONGS:-12}
done
echo "== done $(date -u +%H:%M:%SZ)"
