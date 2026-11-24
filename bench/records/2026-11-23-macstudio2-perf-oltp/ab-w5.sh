#!/usr/bin/env bash
# Lane W5 (oltp): W4's A/B (ab-w4.sh) with four arms on Mac Studio 2: T the old gathering timer
# (98bd0fa9), M main (the candidates' parent), D1 (a flush waits for the batches handed before it)
# and D2 (D1, and after a flush the flusher waits for the senders it told to leave their wait).
# Arms built by ab-w4.sh MODE=build into ~/.cache/amphora-perf-bin/oltp. 5 rounds, the arms' order
# reversed every other round; each round insert_one and update_plain at 1, 32 and 128 clients; each
# run a fresh process on a fresh database; the first arm's calibration under every arm's key (RULES
# 5e). Before and after each run, the load and the CPU all processes take (ps %cpu summed, 100 a core).
set -uo pipefail
ARMS=${ARMS:-"T M D1 D2"}; ROUNDS=${ROUNDS:-5}
HERE=$(cd "$(dirname "$0")" && pwd); PY=$HERE/../2026-11-23-adacachy2-perf-oltp/ab_w3.py
W=$HOME/.cache/oltp-ab; BIN=$HOME/.cache/amphora-perf-bin/oltp; O=${OUT:-$W/out-w5}
state() { echo "load $(sysctl -n vm.loadavg | awk '{print $2, $3, $4}') cpu_pct $(ps -A -o %cpu= | awk '{s+=$1} END {printf "%.0f", s}') ncpu $(sysctl -n hw.ncpu)"; }
[ "$(df -g ~ | tail -1 | awk '{print $4}')" -ge 40 ] || { echo "disk low"; exit 2; }
sha_of() { eval "echo \${$1:?$1 sha}"; }
for a in $ARMS; do [ -x "$BIN/amphora-bench-$(sha_of "$a")" ] || { echo "no binary for $a"; exit 3; }; done
mkdir -p "$O"; rm -f "${O:?}"/*
D=$W/data; mkdir -p "$D"; export XDG_CACHE_HOME=$W/xdg
run() {  # arm-sha op clients label
  rm -rf "${D:?}/x"; local s; s=$(state)
  nice -n 10 "$BIN/amphora-bench-$1" --target amphora --dir "$D/x" --suite --only "$2" --items 200000 --clients "$3" --seconds 1 --warmup 0.5 2>>"$O/runs.err" | tail -1 >"$O/$4.json"
  python3 -I "$PY" line "$O/$4.json" "$4" "$s | after $(state)"
}
echo "$(date -u +%H:%M:%SZ) start $(state)"
first=""
for a in $ARMS; do run "$(sha_of "$a")" 'none$' 1 "load-$a"; [ -n "$first" ] || first=$a; done
python3 -I - "$XDG_CACHE_HOME/amphora/calibration" "$BIN/amphora-bench-$(sha_of "$first")" <<'PY'
import glob, os, sys
d, src = sys.argv[1], sys.argv[2]
texts = {f: open(f).read() for f in glob.glob(os.path.join(d, "*")) if not os.path.basename(f).startswith(".")}
mine = [f for f, t in texts.items() if len(t.split("\n")) > 3 and t.split("\n")[2].startswith("exe " + src + " ")]
print("calibration files", len(texts), "first arm's", len(mine))
if len(mine) == 1:
    body = texts[mine[0]].split("\n", 3)[3]
    for f, t in texts.items():
        open(f, "w").write("\n".join(t.split("\n")[:3]) + "\n" + body)
    print("calibration: the first arm's body written under every arm's key")
PY
rev=$(echo "$ARMS" | awk '{for (i = NF; i > 0; i--) printf "%s ", $i}')
for i in $(seq 1 "$ROUNDS"); do
  if [ $((i % 2)) -eq 1 ]; then order=$ARMS; else order=$rev; fi
  for arm in $order; do
    for op in insert_one update_plain; do for c in 1 32 128; do run "$(sha_of "$arm")" "$op\$" "$c" "r$i-$arm-$op-$c"; done; done
  done
done
python3 -I "$HERE/ab_w5_stats.py" "$O" "$ARMS"
echo "$(date -u +%H:%M:%SZ) done $(state)"
