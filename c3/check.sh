#!/usr/bin/env bash
# Lane C3: build the candidate's lib tests; t_pl_8_13 on three fresh calibrations at once beside the automatic index's probe and the forced shapes; then the sweep. usage: JOBS=4 bash c3/check.sh
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd); cd "$ROOT" || exit 2
. c3/env.sh
out=$(nice -n 10 cargo test -p amphora-engine --lib --locked --no-run 2>&1); rc=$?
echo "$out" | grep -E '^ *(Compiling amphora-engine |error|warning: unused)' | head; echo "build rc=$rc $(date -u +%H:%M:%SZ)"
[ $rc = 0 ] || { echo "$out" | grep -E '^error' -A14 | head -80; exit 4; }
bin=$(echo "$out" | sed -n 's/^ *Executable .*(\(.*\))$/\1/p' | tail -1)
case "$bin" in /*) ;; *) bin="$ROOT/$bin";; esac
P="$BINS/check-$(git rev-parse --short HEAD)"; cp "$bin" "$P"; ls -la "$P"
cd crates/engine || exit 2
L=$HOME/.cache/amphora-perf/planner-c3-logs/check-$(hostname -s); mkdir -p "$L"
pids=""
for i in 1 2 3; do
  Y=$HOME/.cache/amphora-perf/planner-c3-xdg/check$i; rm -rf "${Y:?}"; mkdir -p "$Y"
  ( XDG_CACHE_HOME=$Y nice -n 10 "$P" t_pl_8_13_ --nocapture --test-threads=1 >"$L/t$i.log" 2>&1; echo "t_pl_8_13 round $i rc=$? $(date -u +%H:%M:%SZ)" ) & pids="$pids $!"
done
X=$HOME/.cache/amphora-perf/planner-c3-xdg/check; rm -rf "${X:?}"; mkdir -p "$X"; export XDG_CACHE_HOME=$X
( nice -n 10 "$P" c3_probe_autoindex --nocapture --test-threads=1 >"$L/autoindex.log" 2>&1; echo "autoindex rc=$? $(date -u +%H:%M:%SZ)" ) & pids="$pids $!"
wait $pids
for i in 1 2 3; do echo "--- round $i"; grep -E '^bushy=|panicked|not planned|no LEFT|test result|\.\.\. ' "$L/t$i.log" | cut -c1-420; done
echo "--- automatic index"; grep -E 'C3I|panicked|test result' "$L/autoindex.log" | cut -c1-900
nice -n 10 "$P" c3_sweep --nocapture --test-threads=1 >"$L/sweep.log" 2>&1; echo "sweep rc=$? $(date -u +%H:%M:%SZ)"
grep -E 'C3S|panicked|test result' "$L/sweep.log" | grep -v 'WRONG|' | cut -c1-260
grep -E 'C3S\|WRONG' "$L/sweep.log" | cut -c1-1100 | head -${WRONGS:-24}
echo "== done $(date -u +%H:%M:%SZ)"
