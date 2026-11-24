#!/usr/bin/env bash
# Lane A24: r5 alone, PER processes of each SHA's lib tests at once (every SHA's beside the others, so each meets the
# same load), ROUNDS times; then what each arm counted and its stamps. usage: a24stress.sh ROUNDS PER OUT SHA...
set -u
R=${1:?}; K=${2:?}; OUT=${3:?}; shift 3
B=$HOME/.cache/amphora-perf-bin/cross; mkdir -p "$OUT"
T=database::replicated::tests::session_reads::t_t1_10_r5_a_session_reads_its_write_on_a_lagging_follower
load() { if [ -r /proc/loadavg ]; then cut -d" " -f1-3 /proc/loadavg; else sysctl -n vm.loadavg | tr -d "{}"; fi; }
echo "[$(date -u +%H:%M:%SZ)] start load $(load)"
for r in $(seq 1 "$R"); do
  for k in $(seq 1 "$K"); do
    for s in "$@"; do
      ( nice -n 10 "$B/a24-tests-$s" --exact "$T" --test-threads=1 --show-output > "$OUT/$s-$r-$k.log" 2>&1; echo $? > "$OUT/$s-$r-$k.rc" ) &
    done
  done
  wait
  if [ $((r % 10)) -eq 0 ]; then echo "[$(date -u +%H:%M:%SZ)] round $r load $(load)"; fi
done
echo "[$(date -u +%H:%M:%SZ)] end load $(load)"
python3 - "$OUT" "$@" <<'PY'
import glob, os, re, sys
out, shas = sys.argv[1], sys.argv[2:]
for sha in shas:
    started, rejoined, margin, failed, total, fails = [], [], [], 0, 0, []
    for log in sorted(glob.glob(os.path.join(out, sha + "-*.log"))):
        total += 1
        text = open(log, errors="replace").read()
        rc = open(log[:-4] + ".rc").read().strip()
        m = re.search(r"a24-r5 started_us=(\d+) rejoined_us=(\d+) caught_us=(\d+) answered_us=(\d+)", text)
        if m:
            a, b = int(m.group(1)), int(m.group(2))
            started.append(a); rejoined.append(b); margin.append(b - a)
        if rc != "0":
            failed += 1
            why = [l for l in text.splitlines() if "a24-r5" in l or "panicked at" in l or l.startswith("assertion") or "left:" in l or "right:" in l]
            fails.append((os.path.basename(log), why[:5]))
    q = lambda xs, p: sorted(xs)[min(len(xs) - 1, int(len(xs) * p))] if xs else None
    print(f"{sha[:8]}: {failed} of {total} failed; reader started_us p50={q(started,.5)} p99={q(started,.99)} max={max(started) if started else None}; rejoined_us min={min(rejoined) if rejoined else None} p50={q(rejoined,.5)}; margin (rejoined-started) min={min(margin) if margin else None} p50={q(margin,.5)}")
    for name, why in fails[:4]:
        print("  ", name)
        for line in why:
            print("     ", line[:260])
PY
