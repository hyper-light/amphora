#!/usr/bin/env bash
# Lane C3: build the probe's lib tests and run the sweep. usage: JOBS=4 bash c3/sweep.sh
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd); cd "$ROOT" || exit 2
. c3/env.sh
out=$(nice -n 10 cargo test -p amphora-engine --lib --locked --no-run 2>&1); rc=$?
echo "$out" | grep -E '^ *(Compiling amphora-engine |error|warning: unused)' | head; echo "build rc=$rc $(date -u +%H:%M:%SZ)"
[ $rc = 0 ] || { echo "$out" | grep -E '^error' -A14 | head -80; exit 4; }
bin=$(echo "$out" | sed -n 's/^ *Executable .*(\(.*\))$/\1/p' | tail -1)
case "$bin" in /*) ;; *) bin="$ROOT/$bin";; esac
P="$BINS/sweep-$(git rev-parse --short HEAD)"; cp "$bin" "$P"; ls -la "$P"
cd crates/engine || exit 2
L=$HOME/.cache/amphora-perf/planner-c3-logs/$(hostname -s); mkdir -p "$L"
X=$HOME/.cache/amphora-perf/planner-c3-xdg/sweep; rm -rf "${X:?}"; mkdir -p "$X"; export XDG_CACHE_HOME=$X
nice -n 10 "$P" c3_sweep --nocapture --test-threads=1 >"$L/sweep.log" 2>&1; echo "rc=$?"
grep -E 'C3S|panicked|test result' "$L/sweep.log" | grep -v 'WRONG|' | cut -c1-260
grep -E 'C3S\|WRONG' "$L/sweep.log" | cut -c1-1100 | head -${WRONGS:-24}
echo "== done $(date -u +%H:%M:%SZ)"
