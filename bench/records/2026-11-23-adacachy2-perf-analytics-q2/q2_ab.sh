#!/usr/bin/env bash
# Lane Q2 A/B: S8's statement order (its ten reads, table_count last) at 1/32/128 clients on the Q1
# database (--reuse), main before the fix (A) against after (B), ROUNDS rounds alternated (A,B then
# B,A), Turso and SQLite in every round, the load recorded before each run. One calibration for both
# arms (RULES 5e): B's measured body is written under A's key. usage: q2_ab.sh <A sha> <B sha>
set -uo pipefail
A=$1 B=$2 ROUNDS=${ROUNDS:-5}
BIN=~/.cache/amphora-perf-bin/analytics L=~/.cache/analytics-q2 DATA=~/.cache/analytics-q1/data
ONLY='point_pk$,point_secondary$,point_text$,filter_no_index$,aggregates_range$,join_three$,json_field$,blob_read$,table_group_by$,table_count$'
export XDG_CACHE_HOME=$L/xdg-ab
mkdir -p "$L/ab" "$XDG_CACHE_HOME"
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io_psi $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
fa=$BIN/amphora-bench-$(git -C ~/.cache/amphora-perf/analytics-remote rev-parse "$A") fb=$BIN/amphora-bench-$(git -C ~/.cache/amphora-perf/analytics-remote rev-parse "$B")
[ -x "$fa" ] && [ -x "$fb" ] || { echo "missing binaries $fa $fb"; exit 1; }
run() { # tag binary target
  echo "=== $1 $(date -u +%H:%M:%SZ) $(state)"
  nice -n 10 "$2" --suite --items 200000 --target "$3" --dir "$DATA/$3" --clients 1,32,128 --seconds 1 --warmup 0.5 --only "$ONLY" --reuse 2>/dev/null | tail -1 | python3 -I -c '
import json, sys
r = json.loads(sys.stdin.read()); r["tag"] = sys.argv[1]; r["load"] = open("/proc/loadavg").read().split()[:3]
print(json.dumps(r))' "$1" >> "$L/ab/runs.jsonl"
}
run calib-A "$fa" amphora; run calib-B "$fb" amphora
python3 -I - "$XDG_CACHE_HOME/amphora/calibration" "$fb" <<'PY'
import glob, os, sys
d, src = sys.argv[1], sys.argv[2]
texts = {f: open(f).read() for f in glob.glob(os.path.join(d, "*")) if not os.path.basename(f).startswith(".")}
mine = [f for f, t in texts.items() if t.split("\n")[2].startswith("exe " + src + " ")]
if len(mine) != 1:
    sys.exit(f"calibration: {len(mine)} files for {src} among {len(texts)}")
body = texts[mine[0]].split("\n", 3)[3]
for f, t in texts.items():
    open(f, "w").write("\n".join(t.split("\n")[:3]) + "\n" + body)
    print("calibration", os.path.basename(f), "unified")
PY
for i in $(seq 1 "$ROUNDS"); do
  if [ $((i % 2)) -eq 1 ]; then order="A B"; else order="B A"; fi
  for arm in $order; do
    if [ "$arm" = A ]; then run "A-$i" "$fa" amphora; else run "B-$i" "$fb" amphora; fi
  done
  run "turso-$i" "$fb" turso; run "sqlite-$i" "$fb" sqlite
done
python3 -I - "$L/ab/runs.jsonl" <<'PY'
import collections, json, statistics, sys
rows = collections.defaultdict(list)
for line in open(sys.argv[1]):
    r = json.loads(line); tag = r["tag"].split("-")[0]
    if tag == "calib": continue
    for x in r.get("results", []):
        rows[(x["op"], x["clients"], tag)].append(x.get("ops_per_s") or 0)
for (op, c, tag), v in sorted(rows.items()):
    print(f"{op:18} {c:4} {tag:7} median {statistics.median(v):12.0f} range {min(v):.0f}-{max(v):.0f} n={len(v)}")
PY
echo "=== end $(date -u +%H:%M:%SZ) $(state)"
