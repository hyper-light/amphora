#!/usr/bin/env bash
# Lane T21: T19's fix across the 22 TPC-H queries at SF1, three arms pinned at T17's S weights, all on main a64549cc.
# A = main without a34ec88b's optimizer change (T19's trace kept), B = main, C = B whose filter step keeps only the
# star filters the join search priced (e23ab4b6 merged with main). Each arm's amphora-bench built in the lane's one
# target dir (release); one calibration for all arms (RULES 5e); a reflink copy of T11's db-sf1 per arm. ROUNDS rounds
# of the arms rotated (A B C, B C A, C A B), each a process: every query's counted run then 3 timed runs, each run's
# plan and answer kept. Then, beside the untimed steps, the checks in the background: T19's q03 test on arm A with the
# test added (expected to fail), clippy on amphora-engine at C, and C's join, learned-cost and lip tests. Then SQLite
# once for the answers, answers.py on every arm's every round, each arm traced (a process a query), the summary.
set -uo pipefail
export GIT_TERMINAL_PROMPT=0
A=${A:?} B=${B:?} C=${C:?} RED=${RED:?} ROUNDS=${ROUNDS:-2} STAGES=${STAGES:-build,cal,sweep,checks,sqlite,answers,trace,summary}
R=~/.cache/amphora-perf/tpch-remote W=~/.cache/tpch-ab/t21 WC=~/.cache/tpch-ab/t21-check T=~/.cache/tpch-t11 BIN=~/.cache/amphora-perf-bin/tpch L=~/.cache/tpch-t21
export CARGO_HOME=$HOME/.cache/cargo-home-perf-tpch CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/tpch CARGO_INCREMENTAL=0
BJOBS=${BJOBS:-8} CJOBS=${CJOBS:-6}
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io_psi $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
has() { case ",$STAGES," in *",$1,"*) return 0;; esac; return 1; }
sha_of() { case $1 in A) echo "$A";; B) echo "$B";; C) echo "$C";; esac; }
mkdir -p "$L/out" "$L/ans" "$L/traced" "$BIN"
echo "start $(date -u +%H:%M:%SZ) $(state) nproc $(nproc)"
git -C "$R" fetch -q origin || exit 1
for s in $A $B $C $RED; do git -C "$R" cat-file -e "$s^{commit}" || { echo "missing $s"; exit 1; }; done
[ -d "$W" ] || git -C "$R" worktree add -q --detach "$W" "$B" || exit 1
[ -d "$WC" ] || git -C "$R" worktree add -q --detach "$WC" "$C" || exit 1
if has build; then
  for sha in $B $A $C; do
    [ -x "$BIN/amphora-bench-$sha" ] && { echo "have $sha"; continue; }
    git -C "$W" checkout -q --detach "$sha" || exit 1
    s=$(date +%s); echo "build $sha start $(date -u +%H:%M:%SZ) $(state)"
    (cd "$W/bench" && CARGO_BUILD_JOBS=$BJOBS nice -n 10 cargo build --release --bin amphora-bench > "$L/build-$sha.log" 2>&1) || { grep -E '^(error|warning)' -A14 "$L/build-$sha.log" | head -80; tail -3 "$L/build-$sha.log"; exit 1; }
    grep -E 'Compiling amphora-engine v' "$L/build-$sha.log"
    cp "$CARGO_TARGET_DIR/release/amphora-bench" "$BIN/amphora-bench-$sha" || exit 1
    echo "built $sha in $(( $(date +%s) - s )) s $(state)"
  done
fi
git -C "$W" checkout -q --detach "$B" || exit 1
Q=$W/bench/planner/tpch/queries P=$L/S.json XDG=$L/xdg
cp ~/.cache/tpch-t17/pin/S.json "$P" || exit 1
echo "pin $(sha256sum "$P" | cut -c1-16): $(cat "$P")"
b() { local arm=$1; shift; XDG_CACHE_HOME=$XDG nice -n 10 "$BIN/amphora-bench-$(sha_of "$arm")" --planner --bench tpch --sf 1 --data ~/.cache/planner-bench/tpch-sf1 --dir "$L/db-$arm" --queries "$Q" --max-load 100000 --load-wait-s 0 "$@"; }
if has cal; then
  du -sh "$T/db-sf1" 2>&1 | tail -1
  for arm in A B C; do [ -d "$L/db-$arm" ] || cp -a --reflink=always "$T/db-sf1" "$L/db-$arm" || exit 1; done
  mkdir -p "$XDG"
  for arm in B A C; do s=$(date +%s); b "$arm" --targets amphora --only - --out "$L/out/open-$arm.jsonl" > "$L/out/open-$arm.log" 2>&1; echo "open $arm rc=$? $(( $(date +%s) - s )) s: $(tail -1 "$L/out/open-$arm.log" | cut -c1-200)"; done
  python3 -I - "$XDG/amphora/calibration" "$BIN/amphora-bench-$B" <<'PY'
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
fi
TIMEFORMAT='cpu_s %U %S wall_s %R'
if has sweep; then
  for i in $(seq 1 "$ROUNDS"); do
    case $((i % 3)) in 1) order="A B C";; 2) order="B C A";; *) order="C A B";; esac
    for arm in $order; do
      echo "=== $arm r$i $(date -u +%H:%M:%SZ) $(state)"
      { time b "$arm" --targets amphora --runs 3 --timeout-s 120 --count-timeout-s 120 --pin-weights "$P" --answers "$L/ans/$arm-r$i" --out "$L/out/$arm-r$i.jsonl" 2> "$L/out/$arm-r$i.log"; } 2>&1
      echo "rc=$? $(state)"
    done
  done
fi
checks() {
  cd "$WC" || return 1
  git checkout -q --detach "$RED" || return 1
  echo "red test at $(git log --oneline -1 | cut -c1-80) start $(date -u +%H:%M:%SZ) $(state)"
  CARGO_BUILD_JOBS=$CJOBS nice -n 10 cargo test -p amphora-engine --lib --locked -- optimizer::join::tests::t19_q03 --test-threads=$CJOBS > "$L/red.log" 2>&1
  echo "red test rc=$? (expect 101) $(date -u +%H:%M:%SZ)"; grep -E 'Compiling amphora-engine v|^test |^test result|panicked' "$L/red.log" | head -8; grep -E -A25 '^---- ' "$L/red.log" | head -40
  git checkout -q --detach "$C" || return 1
  echo "clippy at $(git log --oneline -1 | cut -c1-80) start $(date -u +%H:%M:%SZ) $(state)"
  CARGO_BUILD_JOBS=$CJOBS nice -n 10 cargo clippy -p amphora-engine --all-targets --locked -- -D warnings > "$L/clippy-C.log" 2>&1
  echo "clippy rc=$? $(date -u +%H:%M:%SZ)"; grep -E '(Checking|Compiling) amphora-engine v' "$L/clippy-C.log"; tail -1 "$L/clippy-C.log"; grep -E '^(error|warning)' -A14 "$L/clippy-C.log" | head -60
  echo "C tests start $(date -u +%H:%M:%SZ) $(state)"
  CARGO_BUILD_JOBS=$CJOBS nice -n 10 cargo test -p amphora-engine --lib --locked -- optimizer::join learned_costs optimizer::lip --test-threads=$CJOBS > "$L/test-C.log" 2>&1
  echo "C tests rc=$? $(date -u +%H:%M:%SZ)"; grep -E 'Compiling amphora-engine v|^test result|FAILED|panicked|t19_' "$L/test-C.log" | head -20; grep -E -A30 '^---- ' "$L/test-C.log" | head -60
}
if has checks; then checks > "$L/checks.txt" 2>&1 & CHECKS_PID=$!; echo "checks started pid $CHECKS_PID $(date -u +%H:%M:%SZ)"; fi
if has sqlite; then
  echo "=== sqlite $(date -u +%H:%M:%SZ) $(state)"
  { time XDG_CACHE_HOME=$XDG nice -n 10 "$BIN/amphora-bench-$B" --planner --bench tpch --sf 1 --data ~/.cache/planner-bench/tpch-sf1 --dir "$L/db-B" --queries "$Q" --max-load 100000 --load-wait-s 0 --targets sqlite --runs 1 --timeout-s 300 --answers "$L/ans/sqlite" --out "$L/out/sqlite.jsonl" 2> "$L/out/sqlite.log"; } 2>&1
  echo "rc=$? $(state)"
fi
if has answers; then
  for d in "$L"/ans/[ABC]-r*; do
    for s in "$L"/ans/sqlite/*/*/sqlite.tsv; do q=$(dirname "$s"); cp "$s" "$d/$(basename "$(dirname "$q")")/$(basename "$q")/" 2>/dev/null; done
    echo "$(basename "$d"): $(python3 -I "$W/bench/planner/answers.py" "$d" 2>&1 | tail -3 | tr '\n' ' ')"
  done
fi
if has trace; then
  echo "=== traced $(date -u +%H:%M:%SZ) $(state)"
  for arm in A B C; do
    for q in $(ls "$Q" | sed -n 's/\.sql$//p'); do
      AMPHORA_PLAN_TRACE=1 AMPHORA_SEEK_TRACE=1 b "$arm" --targets amphora --only "$q" --runs 1 --timeout-s 120 --count-timeout-s 120 --pin-weights "$P" --out "$L/traced/$arm-$q.jsonl" 2> "$L/traced/$arm-$q.log" || echo "traced $arm $q rc=$?"
    done
    echo "traced $arm done $(date -u +%H:%M:%SZ) $(state) $(du -sh "$L/traced" | cut -f1)"
  done
fi
if has checks; then wait "$CHECKS_PID"; echo "=== checks"; cat "$L/checks.txt"; fi
if has summary; then python3 -I "$L/summ21.py" "$L" > "$L/summary.txt" 2>&1; echo "=== summary ($(wc -l < "$L/summary.txt") lines)"; head -c 20000 "$L/summary.txt"; fi
echo "=== end $(date -u +%H:%M:%SZ) $(state)"
