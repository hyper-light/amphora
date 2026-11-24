#!/usr/bin/env bash
# Lane Q3 A/B: the five point statements at 1, 32 and 128 clients, main before the change (A)
# against after (B), ROUNDS rounds alternated (A,B then B,A), Turso, SQLite and PostgreSQL 19 in every
# round, the load and CPU/I-O pressure recorded before each run. The file databases are loaded once
# (A's binary) into the lane's own folder and reopened (--reuse) by every timed run; PostgreSQL 19 runs
# in the lane's own container (port 55491, servers/postgres.sh's settings) and loads its table each run.
# One calibration for both arms (RULES 5e): B's measured body is written under A's key.
# usage: q3_ab.sh <A sha> <B sha>
set -uo pipefail
A=$1 B=$2 ROUNDS=${ROUNDS:-5}
BIN=~/.cache/amphora-perf-bin/analytics L=~/.cache/analytics-q3 DATA=~/.cache/analytics-q3/ab-data PORT=55491 NAME=q3-reads-pg19
ONLY='point_pk$,point_secondary$,point_text$,json_field$,blob_read$'
ARGS=(--suite --items 200000 --clients 1,32,128 --seconds 1 --warmup 0.5 --only "$ONLY")
export XDG_CACHE_HOME=$L/xdg-ab
mkdir -p "$L/ab" "$DATA" "$XDG_CACHE_HOME" "$L/pg19"
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io_psi $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
R=~/.cache/amphora-perf/analytics-remote
fa=$BIN/amphora-bench-$(git -C "$R" rev-parse "$A") fb=$BIN/amphora-bench-$(git -C "$R" rev-parse "$B")
[ -x "$fa" ] && [ -x "$fb" ] || { echo "missing binaries $fa $fb"; exit 1; }
docker rm -f "$NAME" >/dev/null 2>&1
docker run -d --name "$NAME" --network host --user "$(id -u):$(id -g)" --memory 8g --memory-swap 8g --shm-size 256m \
  -e POSTGRES_HOST_AUTH_METHOD=trust -v "$L/pg19:/var/lib/postgresql" postgres:19 \
  postgres -c listen_addresses=127.0.0.1 -c port=$PORT -c max_connections=1100 >/dev/null || exit 1
trap 'docker rm -f "$NAME" >/dev/null 2>&1' EXIT
for _ in $(seq 1 60); do docker exec "$NAME" pg_isready -q -h 127.0.0.1 -p $PORT -U postgres && break; sleep 1; done
docker exec "$NAME" psql -h 127.0.0.1 -p $PORT -U postgres -AtX -c "select version()"
run() { # tag binary target [extra]
  local tag=$1 bin=$2 t=$3; shift 3
  echo "=== $tag $(date -u +%H:%M:%SZ) $(state)"
  local before; before=$(state)
  nice -n 10 "$bin" "${ARGS[@]}" --target "$t" --dir "$DATA/$t" "$@" 2>>"$L/ab/err.log" | tail -1 | python3 -I -c '
import json, sys
r = json.loads(sys.stdin.read()); r["tag"] = sys.argv[1]; r["state_before"] = sys.argv[2]; r["load"] = open("/proc/loadavg").read().split()[:3]
print(json.dumps(r))' "$tag" "$before" >> "$L/ab/runs.jsonl"
}
for t in amphora turso sqlite; do run "load-$t" "$fa" "$t" --clients 1 --seconds 0.2 --warmup 0; done
run calib-A "$fa" amphora --reuse; run calib-B "$fb" amphora --reuse
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
    if [ "$arm" = A ]; then run "A-$i" "$fa" amphora --reuse; else run "B-$i" "$fb" amphora --reuse; fi
  done
  run "turso-$i" "$fb" turso --reuse; run "sqlite-$i" "$fb" sqlite --reuse
  run "postgres-$i" "$fb" postgres --dsn "postgres://postgres@127.0.0.1:$PORT/postgres"
done
python3 -I - "$L/ab/runs.jsonl" <<'PY'
import collections, json, statistics, sys
rows = collections.defaultdict(list)
for line in open(sys.argv[1]):
    r = json.loads(line); tag = r["tag"].split("-")[0]
    if tag in ("calib", "load"): continue
    for x in r.get("results", []):
        rows[(x["op"], x["clients"], tag)].append((x.get("ops_per_s") or 0, x.get("errors") or 0))
for (op, c, tag), v in sorted(rows.items()):
    o = [a for a, _ in v]
    print(f"{op:16} {c:4} {tag:9} median {statistics.median(o):12.0f} range {min(o):.0f}-{max(o):.0f} n={len(o)} errors={sum(e for _, e in v)}")
PY
echo "=== end $(date -u +%H:%M:%SZ) $(state)"
