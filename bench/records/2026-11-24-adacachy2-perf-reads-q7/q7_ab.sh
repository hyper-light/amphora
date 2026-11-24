#!/usr/bin/env bash
# Lane Q7: lane Q6's step 4 (q6_ab.sh) run: main before the record registry's read lock (A) against
# the fix (B), K0's mixed phase at 1, 32 and 128 clients, on adacachy2. Changes from q6_ab.sh: tools
# taken from git at TOOLSHA; B fetched from perf/analytics too (its code is what main fast-forwards
# to); 1 client added; events/s is the bench's write_events_per_s (q6_ab.sh divided writes counted
# after the warm-up by the whole phase); cores used and each run's load printed as it ends; q7_limit.py
# beside q6_split.py; rounds stop at DEADLINE.
#   1. each arm built from its SHA in lane Q6's worktree, one after the other in the lane's target dir
#      (RULES 5c), copied to amphora-perf-bin;
#   2. one profile an arm at 32 and 128 clients (AMPHORA_PROFILE, q6_sched.py beside it);
#   3. one calibration for both arms (RULES 5e): A's measured body under B's key;
#   4. timed rounds, A,B then B,A, the load and CPU and I/O pressure before each run;
#   5. every timed run's answers held to SQLite's (q5_answers.py, as lane Q5's r75); then the table.
# usage: A=<sha> B=<sha> TOOLSHA=<sha> DEADLINE=<epoch s> JOBS=<n> q7_ab.sh
set -uo pipefail
A=${A:?} B=${B:?} TOOLSHA=${TOOLSHA:?} DEADLINE=${DEADLINE:?} ROUNDS=${ROUNDS:-5} JOBS=${JOBS:-8}
L=~/.cache/analytics-q7 SRC=~/.cache/analytics-q6/src T=~/.cache/amphora-perf-target/analytics R=~/.cache/amphora-perf/analytics-remote
BIN=~/.cache/amphora-perf-bin/analytics H=~/.cache/cargo-home-perf-analytics TOOLS=$L/tools
RQ=bench/records/2026-11-24-adacachy2-perf-reads
export XDG_CACHE_HOME=$L/xdg-ab
mkdir -p "$L/k0" "$L/prof" "$BIN" "$XDG_CACHE_HOME" "$TOOLS" || exit 1
: > "$L/ab.jsonl"; : > "$L/ab-answers.jsonl"
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io_psi $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
echo "=== start $(date -u +%H:%M:%SZ) $(state) cores $(nproc) jobs $JOBS"
ps -eo pcpu,nlwp,comm --sort=-pcpu | head -8
git -C "$R" fetch -q origin main perf/analytics || exit 1
for f in q6-q6_profile.sh q6-q6_sched.py q6-q6_split.py q5-q5_answers.py q7-q7_limit.py; do
  git -C "$R" show "$TOOLSHA:$RQ-${f%%-*}/${f#*-}" > "$TOOLS/${f#*-}" || exit 1
done
[ -d "$SRC" ] || git -C "$R" worktree add -q --detach "$SRC" "$A" || exit 1
[ -d "$H" ] || { mkdir -p "$H" && for d in registry git bin; do ln -s ~/.cargo/$d "$H/$d"; done; }
for sha in "$A" "$B"; do
  [ -x "$BIN/amphora-bench-$sha" ] && continue
  git -C "$SRC" checkout -q --detach "$sha" || exit 1
  echo "=== build $sha $(date -u +%H:%M:%SZ) $(state)"
  (cd "$SRC/bench" && env CARGO_HOME="$H" CARGO_TARGET_DIR="$T" CARGO_BUILD_JOBS="$JOBS" CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true \
    nice -n 10 cargo build --release > "$L/build-${sha:0:8}.log" 2>&1); rc=$?
  grep -E '^error|Compiling amphora-engine |Finished' "$L/build-${sha:0:8}.log" | tail -3
  [ "$rc" = 0 ] && [ "$(git -C "$SRC" rev-parse HEAD)" = "$sha" ] || { echo "build of $sha failed ($rc)"; tail -20 "$L/build-${sha:0:8}.log"; exit 1; }
  cp "$T/release/amphora-bench" "$BIN/amphora-bench-$sha" || exit 1
done
[ "$(sha256sum < "$BIN/amphora-bench-$A")" != "$(sha256sum < "$BIN/amphora-bench-$B")" ] || { echo "A and B are the same binary"; exit 1; }
source <(sed -n '/^prof() {/,/^}/p' "$TOOLS/q6_profile.sh")
for n in 32 128; do for arm in "$A" "$B"; do prof "ab-${arm:0:8}-$n" "$BIN/amphora-bench-$arm" "$n"; done; done
python3 -I "$TOOLS/q6_split.py" "$L"/prof/ab-*.folded
python3 -I "$TOOLS/q7_limit.py" --top 15 "$L"/prof/ab-*.folded
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
  env AMPHORA_ANSWERS="$dir.answers" nice -n 10 "$BIN/amphora-bench-$arm" --target amphora --dir "$dir" \
    --sessions 1000 --preload 50 --writers "$n" --readers "$n" --seconds 6 --warmup 1 --searches 0 > "$dir.out" 2>>"$L/err-ab.log"; rc=$?
  python3 -I -c '
import json, sys
try:
    r = json.loads(open(sys.argv[5]).read().strip().splitlines()[-1])
except Exception:
    r = {"error": "no report"}
print(json.dumps({"tag": sys.argv[1], "arm": sys.argv[2], "clients": int(sys.argv[3]), "state_before": sys.argv[4], "exit": int(sys.argv[6]), "report": r}))' "$tag" "$arm" "$n" "$before" "$dir.out" "$rc" | tee -a "$L/ab.jsonl" | python3 -I -c '
import json, sys
x = json.loads(sys.stdin.read()); r = x["report"]
ok = x["exit"] == 0 and isinstance(r.get("read_ms"), dict)
print(x["tag"][:24], "exit", x["exit"], "reads/s %.0f events/s %.0f cores %.1f failures %s unanswered %s" % (r["read_ms"]["n"] / r["seconds"], r["write_events_per_s"], (r.get("mixed_cpu_s") or 0) / r["seconds"], r.get("failures"), r.get("unanswered")) if ok else r, "|", x["state_before"])'
  [ "$rc" = 0 ] && python3 -I "$TOOLS/q5_answers.py" "$tag" "$dir.answers" | tee -a "$L/ab-answers.jsonl" | cut -c1-160
  rm -rf "${dir:?}" "$dir.answers"
}
for i in $(seq 1 "$ROUNDS"); do
  [ $(( DEADLINE - $(date +%s) )) -gt 150 ] || { echo "=== round $i not started: deadline"; break; }
  if [ $((i % 2)) -eq 1 ]; then order="$A $B"; else order="$B $A"; fi
  echo "=== round $i $(date -u +%H:%M:%SZ)"
  for n in 1 32 128; do for arm in $order; do timed "${arm:0:8}-$i-$n" "$arm" "$n"; done; done
done
python3 -I - "$L/ab.jsonl" "$L/ab-answers.jsonl" "$A" "$B" <<'PY'
import itertools, json, statistics, sys
runs = [json.loads(l) for l in open(sys.argv[1])]
answers = [json.loads(l) for l in open(sys.argv[2])]
a, b = sys.argv[3], sys.argv[4]
def ranksum_p(x, y):
    # exact two-sided Mann-Whitney U over every split of the pooled values
    pooled, k = x + y, len(x)
    if not x or not y:
        return float("nan")
    u = lambda xs, ys: sum((p > q) + 0.5 * (p == q) for p in xs for q in ys)
    obs = abs(u(x, y) - len(x) * len(y) / 2)
    splits = list(itertools.combinations(range(len(pooled)), k))
    hits = sum(1 for c in splits if abs(u([pooled[i] for i in c], [pooled[i] for i in range(len(pooled)) if i not in c]) - len(x) * len(y) / 2) >= obs - 1e-9)
    return hits / len(splits)
print("| clients | arm | runs | reads/s median (range) | events/s median (range) | cores median | read p50 / p99 ms | failures | unanswered | failed runs |")
print("|---|---|---|---|---|---|---|---|---|---|")
got = {}
for n in (1, 32, 128):
    for arm in (a, b):
        rs = [r for r in runs if r["clients"] == n and r["arm"] == arm]
        ok = [r["report"] for r in rs if r["exit"] == 0 and isinstance(r["report"].get("read_ms"), dict)]
        reads = [x["read_ms"]["n"] / x["seconds"] for x in ok]
        events = [x["write_events_per_s"] for x in ok]
        cores = [(x.get("mixed_cpu_s") or 0) / x["seconds"] for x in ok]
        got[(n, arm)] = (reads, events)
        p = lambda k: statistics.median(x["read_ms"][k] for x in ok) if ok else float("nan")
        rng = lambda v: f"{statistics.median(v):,.0f} ({min(v):,.0f}-{max(v):,.0f})" if v else "-"
        print(f"| {n} | {arm[:8]} | {len(ok)} | {rng(reads)} | {rng(events)} | {statistics.median(cores) if cores else float('nan'):.1f} | {p('p50'):.3f} / {p('p99'):.3f} | {sum(x.get('failures', 0) for x in ok)} | {sum(x.get('unanswered', 0) or 0 for x in ok)} | {len(rs) - len(ok)} |")
for n in (1, 32, 128):
    (ra, ea), (rb, eb) = got[(n, a)], got[(n, b)]
    ratio = lambda x, y: statistics.median(y) / statistics.median(x) if x and y and statistics.median(x) else float("nan")
    print(f"{n} clients: B/A reads {ratio(ra, rb):.2f}x (rank-sum p {ranksum_p(ra, rb):.3f}), events {ratio(ea, eb):.2f}x (p {ranksum_p(ea, eb):.3f})")
print(f"answers held to SQLite: {len(answers)} runs, {sum(x['sessions'] for x in answers)} pages, {sum(x['differ'] for x in answers)} differ")
PY
echo "=== end $(date -u +%H:%M:%SZ) $(state)"
