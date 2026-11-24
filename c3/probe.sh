#!/usr/bin/env bash
# Lane C3: build the probe's lib tests, run t_pl_8_13 on three fresh calibrations, then the probe. usage: JOBS=4 bash c3/probe.sh
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd); cd "$ROOT" || exit 2
. c3/env.sh
out=$(nice -n 10 cargo test -p amphora-engine --lib --locked --no-run 2>&1); rc=$?
echo "$out" | grep -E '^ *(Compiling amphora-engine |error|warning: unused)' | head; echo "build rc=$rc $(date -u +%H:%M:%SZ)"
[ $rc = 0 ] || { echo "$out" | grep -E '^error' -A14 | head -80; exit 4; }
bin=$(echo "$out" | sed -n 's/^ *Executable .*(\(.*\))$/\1/p' | tail -1)
case "$bin" in /*) ;; *) bin="$ROOT/$bin";; esac
P="$BINS/probe-$(git rev-parse --short HEAD)"; cp "$bin" "$P"; ls -la "$P"
cd crates/engine || exit 2
L=$HOME/.cache/amphora-perf/planner-c3-logs/$(hostname -s); mkdir -p "$L"
echo "--- t_pl_8_13, three fresh calibrations"
for i in 1 2 3; do
  Y=$HOME/.cache/amphora-perf/planner-c3-xdg/t$i; rm -rf "${Y:?}"; mkdir -p "$Y"
  XDG_CACHE_HOME=$Y nice -n 10 "$P" t_pl_8_13_a_left_join_inside --nocapture --test-threads=1 2>&1 | cut -c1-900 | tail -25
done
X=$HOME/.cache/amphora-perf/planner-c3-xdg/probe; rm -rf "${X:?}"; mkdir -p "$X"; export XDG_CACHE_HOME=$X
echo "--- answers against SQLite"
C3_DIR=$L/answers nice -n 10 "$P" c3_probe_answers --nocapture --test-threads=1 >"$L/answers.log" 2>&1; echo "rc=$?"; tail -3 "$L/answers.log" | cut -c1-300
python3 "$ROOT/c3/judge.py" "$L/answers" 2>&1 | cut -c1-600
echo "--- forced shapes"
nice -n 10 "$P" c3_probe_forced --nocapture --test-threads=1 2>&1 | cut -c1-1200
echo "--- plans weighed"
RUST_LOG=amphora::join_search=debug nice -n 10 "$P" c3_probe_weighed --nocapture --test-threads=1 >"$L/weighed.log" 2>&1; echo "rc=$?"; grep -c weighed "$L/weighed.log"; grep -E 'C3W' "$L/weighed.log" | cut -c1-900
echo "--- an automatic index's build"
nice -n 10 "$P" c3_probe_autoindex --nocapture --test-threads=1 2>&1 | grep -E 'C3I|panicked|test result' | cut -c1-900
echo "== done $(date -u +%H:%M:%SZ)"
