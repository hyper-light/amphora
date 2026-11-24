#!/usr/bin/env bash
# Lane Q1, step 1: table_count's evidence (plans, bytecode, profile, per-thread wait trace, reference numbers)
# on the binary S8's numbers came from (S7's 795faa83; S8's 701486af changed shell only), or $BIN.
set -uo pipefail
L=~/.cache/analytics-q1 BIN=${BIN:-$HOME/.cache/amphora-perf-bin/bench-s7/amphora-bench-795faa8377d79ffe4d82f3eb7cb19717453c3a9e}
TAG=${TAG:-s7}
mkdir -p "$L"
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io_psi $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
export XDG_CACHE_HOME=$L/xdg
b() { nice -n 10 "$BIN" --suite --items 200000 "$@"; }
summ() { python3 -I -c '
import json, sys
r = json.loads(sys.stdin.read().strip().splitlines()[-1])
for x in r.get("results", []):
    ms = x.get("ms") or {}
    print("  ", r["target"], x["op"], "clients", x["clients"], "ops/s", round(x.get("ops_per_s") or 0), "p50", ms.get("p50"), "p99", ms.get("p99"), "cpu_user_s", x.get("cpu_user_s"), "cpu_sys_s", x.get("cpu_sys_s"), "errors", x.get("errors"), "rows/op", x.get("rows_per_op"))
'; }
if [ "${SKIP_LOAD:-0}" != 1 ]; then
  echo "=== explain + load $(date -u +%H:%M:%SZ) $(state)"
  for t in amphora turso sqlite; do
    b --target $t --dir $L/data/$t --clients 1 --explain --only 'table_count$' 2>$L/load-$t.err | python3 -I -c 'import json,sys; r=json.load(sys.stdin); print(r["target"], json.dumps(r["plans"]))'
  done
  AMPHORA_EQP_JSON=1 b --target amphora --dir $L/data/amphora --clients 1 --explain --only 'table_count$' --reuse 2>/dev/null | python3 -I -c 'import json,sys; r=json.load(sys.stdin); print("amphora json", json.dumps(r["plans"])[:1500])'
fi
echo "=== query + bytecode $(date -u +%H:%M:%SZ) $(state)"
for t in amphora sqlite; do
  nice -n 10 "$BIN" --query --target $t --dir $L/data/$t --sql 'SELECT COUNT(*) FROM items' --explain --repeat 12 2>&1 | head -50
done
python3 -I - "$L/data/sqlite/suite.db" "$L/data/amphora/suite.db" <<'PY'
import sqlite3, sys
for f in sys.argv[1:]:
    try:
        c = sqlite3.connect(f"file:{f}?mode=ro", uri=True)
        print(f, "page_size", c.execute("pragma page_size").fetchone(), "page_count", c.execute("pragma page_count").fetchone())
        for row in c.execute("select name, count(*), sum(pagetype='leaf'), sum(ncell) from dbstat group by name order by 2 desc"):
            print("  dbstat", row)
    except Exception as e:
        print(f, "error", e)
PY
echo "=== profile $(date -u +%H:%M:%SZ) $(state)"
AMPHORA_PROFILE=$L/prof-$TAG b --target amphora --dir $L/data/amphora --clients 1,32,128 --seconds 1 --warmup 0.5 --only 'table_count$' --reuse 2>$L/prof-$TAG.err | summ
for c in 1 32 128; do
  f=$L/prof-$TAG-amphora-table_count-$c.txt
  echo "--- profile $c clients ($f)"
  sed -n '/== threads ==/,/^$/p' "$f" | head -8
  sed -n '/== all: samples/,/^$/p' "$f" | head -28
  sed -n '/== all: inclusive/,/^$/p' "$f" | head -50
done
echo "=== trace $(date -u +%H:%M:%SZ) $(state)"
python3 -I $L/q1_sample.py $L/trace-$TAG.json nice -n 10 "$BIN" --suite --items 200000 --target amphora --dir $L/data/amphora --clients 1,32,128 --seconds 1 --warmup 0.5 --only 'table_count$' --reuse 2>$L/trace-$TAG.err
echo "=== timed $(date -u +%H:%M:%SZ) $(state)"
for t in amphora turso sqlite; do
  b --target $t --dir $L/data/$t --clients 1,32,128 --seconds 1 --warmup 0.5 --only 'table_count$' --reuse 2>/dev/null | summ
  echo "   $(state)"
done
echo "=== end $(date -u +%H:%M:%SZ) $(state)"
