#!/usr/bin/env bash
# Lane Q6, step 4, not run in the lane's hour (adacachy2 was the verifier's from 03:00Z): main
# before the record registry's read lock (A) against main with it (B), K0's mixed phase at 32 and
# 128 clients, on adacachy2.
#   1. each arm built from its SHA in the lane's worktree ($L/src: a checkout gives changed files a
#      new mtime, so cargo rebuilds them; an archive's commit-time mtimes could leave B built as A),
#      one after the other in the lane's target dir (RULES 5c), copied to amphora-perf-bin;
#   2. one profile an arm a client count (AMPHORA_PROFILE, q6_sched.py beside it), split by q6_split.py;
#   3. one calibration for both arms (RULES 5e, as lane T5's ab.sh): A's measured body under B's key;
#   4. ROUNDS timed rounds, A,B then B,A, the load and CPU and I/O pressure before each run;
#   5. every timed run's answers held to SQLite's (q5_answers.py, as lane Q5's r75); then the table.
# usage: A=<sha> B=<sha> TOOLS=<dir with q6_profile.sh q6_sched.py q6_split.py q5_answers.py> q6_ab.sh
set -uo pipefail
A=${A:?} B=${B:?} ROUNDS=${ROUNDS:-5}
L=~/.cache/analytics-q6 T=~/.cache/amphora-perf-target/analytics R=~/.cache/amphora-perf/analytics-remote
BIN=~/.cache/amphora-perf-bin/analytics H=~/.cache/cargo-home-perf-analytics
export XDG_CACHE_HOME=$L/xdg-ab
mkdir -p "$L/k0" "$L/prof" "$BIN" "$XDG_CACHE_HOME" || exit 1
: > "$L/ab.jsonl"; : > "$L/ab-answers.jsonl"
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io_psi $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
git -C "$R" fetch -q origin main || exit 1
[ -d "$L/src" ] || git -C "$R" worktree add -q --detach "$L/src" "$A" || exit 1
for sha in "$A" "$B"; do
  [ -x "$BIN/amphora-bench-$sha" ] && continue
  git -C "$L/src" checkout -q --detach "$sha" || exit 1
  (cd "$L/src/bench" && env CARGO_HOME="$H" CARGO_TARGET_DIR="$T" CARGO_BUILD_JOBS="${JOBS:-4}" CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true \
    nice -n 10 cargo build --release 2>&1 | grep -E '^error|Compiling amphora-engine |Finished' | tail -3)
  cp "$T/release/amphora-bench" "$BIN/amphora-bench-$sha" || exit 1
done
[ "$(sha256sum < "$BIN/amphora-bench-$A")" != "$(sha256sum < "$BIN/amphora-bench-$B")" ] || { echo "A and B are the same binary"; exit 1; }
source <(sed -n '/^prof() {/,/^}/p' "$TOOLS/q6_profile.sh")
for n in 32 128; do for arm in "$A" "$B"; do prof "ab-$arm-$n" "$BIN/amphora-bench-$arm" "$n"; done; done
python3 -I "$TOOLS/q6_split.py" "$L"/prof/ab-*.folded
python3 -I - "$XDG_CACHE_HOME/amphora/calibration" "$BIN/amphora-bench-$A" <<'PY'
import glob, hashlib, os, sys
d, src = sys.argv[1], sys.argv[2]
texts = {f: open(f).read() for f in glob.glob(os.path.join(d, "*")) if not os.path.basename(f).startswith(".")}
mine = [f for f, t in texts.items() if t.split("\n")[2].startswith("exe " + src + " ")]
if len(mine) != 1:
    sys.exit(f"calibration: {len(mine)} files for {src} among {len(texts)}")
body = texts[mine[0]].split("\n", 3)[3]
for f, t in texts.items():
    open(f, "w").write("\n".join(t.split("\n")[:3]) + "\n" + body)
    print("calibration", os.path.basename(f), t.split("\n")[2].split(" ")[1][-40:], "body sha256", hashlib.sha256(body.encode()).hexdigest()[:16])
PY
timed() { # tag arm clients
  local tag=$1 arm=$2 n=$3 dir=$L/k0/$1 before rc
  rm -rf "${dir:?}"; before=$(state)
  echo "=== $tag $(date -u +%H:%M:%SZ) $before"
  env AMPHORA_ANSWERS="$dir.answers" nice -n 10 "$BIN/amphora-bench-$arm" --target amphora --dir "$dir" \
    --sessions 1000 --preload 50 --writers "$n" --readers "$n" --seconds 6 --warmup 1 --searches 0 > "$dir.out" 2>>"$L/err-ab.log"; rc=$?
  python3 -I -c '
import json, sys
try:
    r = json.loads(open(sys.argv[5]).read().strip().splitlines()[-1])
except Exception:
    r = {"error": "no report"}
print(json.dumps({"tag": sys.argv[1], "arm": sys.argv[2], "clients": int(sys.argv[3]), "state_before": sys.argv[4], "exit": int(sys.argv[6]), "report": r}))' "$tag" "$arm" "$n" "$before" "$dir.out" "$rc" >> "$L/ab.jsonl"
  [ "$rc" = 0 ] && python3 -I "$TOOLS/q5_answers.py" "$tag" "$dir.answers" >> "$L/ab-answers.jsonl"
  rm -rf "${dir:?}" "$dir.answers"
}
for i in $(seq 1 "$ROUNDS"); do
  if [ $((i % 2)) -eq 1 ]; then order="$A $B"; else order="$B $A"; fi
  for n in 32 128; do for arm in $order; do timed "$arm-$i-$n" "$arm" "$n"; done; done
done
python3 -I - "$L/ab.jsonl" "$L/ab-answers.jsonl" "$A" "$B" <<'PY'
import json, statistics, sys
runs = [json.loads(l) for l in open(sys.argv[1])]
answers = [json.loads(l) for l in open(sys.argv[2])]
a, b = sys.argv[3], sys.argv[4]
print("| clients | arm | reads/s median (range) | events/s median (range) | read p50 / p99 ms | failures | failed runs |")
print("|---|---|---|---|---|---|---|")
for n in (32, 128):
    for arm in (a, b):
        rs = [r for r in runs if r["clients"] == n and r["arm"] == arm]
        ok = [r["report"] for r in rs if r["exit"] == 0 and "read_ms" in r["report"]]
        reads = [x["read_ms"]["n"] / x["seconds"] for x in ok]
        events = [x["events_written"] / x["seconds"] for x in ok]
        p = lambda k: statistics.median(x["read_ms"][k] for x in ok) if ok else float("nan")
        rng = lambda v: f"{statistics.median(v):,.0f} ({min(v):,.0f}-{max(v):,.0f})" if v else "-"
        print(f"| {n} | {arm[:8]} | {rng(reads)} | {rng(events)} | {p('p50'):.3f} / {p('p99'):.3f} | {sum(x.get('failures', 0) for x in ok)} | {len(rs) - len(ok)} |")
print(f"answers held to SQLite: {len(answers)} runs, {sum(x['sessions'] for x in answers)} pages, {sum(x['differ'] for x in answers)} differ")
PY
echo "=== end $(date -u +%H:%M:%SZ) $(state)"
