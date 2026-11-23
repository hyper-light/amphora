#!/usr/bin/env bash
# Lane W1 (oltp): alternated A/B of insert_one at 1 client. A = main 5f511d8d, B = e57a028b (perf/oltp)
# (the suite's SqlEngine reports Engine::uncopied). 5 rounds (A,B then B,A), each run a fresh
# process on a fresh database, SQLite once a round beside them. Then clippy on the bench crate.
set -uo pipefail
A=5f511d8deb1218ffb8a800095bd59b96839d158a
L=$HOME/.cache/amphora-perf/oltp-run; R=$HOME/.cache/amphora-perf/oltp-remote; BIN=$HOME/.cache/amphora-perf-bin/oltp
O=$L/ab-e57a028b; mkdir -p "$O"
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io_psi $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
export CARGO_HOME=$HOME/.cache/cargo-home-perf-oltp CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/oltp CARGO_BUILD_JOBS=8 CARGO_INCREMENTAL=0 \
  CARGO_PROFILE_RELEASE_LTO=thin CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=true CARGO_PROFILE_RELEASE_DEBUG=line-tables-only
cd "$R" || exit 3
git fetch -q origin perf/oltp && git checkout -q --detach e57a028b8f99127179eaf0b4e24450373479cb2e || { echo "checkout B failed"; exit 3; }
echo "B: $(git rev-parse HEAD)"
echo "$(date -u +%H:%M:%SZ) build-B $(state)"
nice -n 10 cargo build --release --offline --manifest-path bench/Cargo.toml --bin amphora-bench >"$O/build.log" 2>&1 || { tail -20 "$O/build.log"; exit 4; }
cp "$CARGO_TARGET_DIR/release/amphora-bench" "$BIN/amphora-bench-B-e57a028b"
echo "$(date -u +%H:%M:%SZ) built $(state)"
D=$L/data-ab; mkdir -p "$D" && chattr +C "$D" 2>/dev/null
export XDG_CACHE_HOME=$L/xdg-ab
ARGS=(--suite --only 'insert_one$' --items 200000 --clients 1 --seconds 1 --warmup 0.5)
one() {  # arm target label
  local exe=$BIN/amphora-bench-$1; rm -rf "${D:?}/x"
  local s; s=$(state)
  nice -n 10 "$exe" --target "$2" --dir "$D/x" "${ARGS[@]}" 2>>"$O/runs.err" | tail -1 >"$O/$3.json"
  python3 -I -c 'import json,sys; r=json.load(open(sys.argv[1])); x=r["results"][0]; st=r.get("stats") or {}; f=st.get("flushes") or 0
print(sys.argv[2], "ops/s", x["ops_per_s"], "p50", x["ms"]["p50"], "p90", x["ms"]["p90"], "p99", x["ms"]["p99"], "n", x["ms"]["n"], "errors", x["errors"],
 "flush_ms/flush", round(st["flush_ms"]/f, 2) if f else "-", "flushes", f, "commits", st.get("commits", "-"), "checkpoints", st.get("checkpoints", "-"), "checkpoint_ms", st.get("checkpoint_ms", "-"), "apply_ms", st.get("apply_ms", "-"), sys.argv[3])' "$O/$3.json" "$3" "$s"
}
one "$A" amphora warm-A; one B-e57a028b amphora warm-B
python3 -I - "$XDG_CACHE_HOME/amphora/calibration" "$BIN/amphora-bench-$A" <<'PY'
import glob, os, sys
d, src = sys.argv[1], sys.argv[2]
texts = {f: open(f).read() for f in glob.glob(os.path.join(d, "*")) if not os.path.basename(f).startswith(".")}
mine = [f for f, t in texts.items() if len(t.split("\n")) > 3 and t.split("\n")[2].startswith("exe " + src + " ")]
print("calibration files", len(texts), "A's", len(mine))
if len(mine) == 1:
    body = texts[mine[0]].split("\n", 3)[3]
    for f, t in texts.items():
        open(f, "w").write("\n".join(t.split("\n")[:3]) + "\n" + body)
    print("calibration: A's body written under every arm's key")
PY
for i in 1 2 3 4 5; do
  if [ $((i % 2)) -eq 1 ]; then order="$A B-e57a028b"; else order="B-e57a028b $A"; fi
  for arm in $order; do one "$arm" amphora "r$i-$( [ "$arm" = "$A" ] && echo A || echo B )"; done
  one "$A" sqlite "r$i-sqlite"
done
python3 -I - "$O" <<'PY'
import glob, json, os, statistics, sys
o = sys.argv[1]
for arm in ("A", "B", "sqlite"):
    rs = []
    for f in sorted(glob.glob(os.path.join(o, f"r*-{arm}.json"))):
        r = json.load(open(f)); x = r["results"][0]; st = r.get("stats") or {}
        rs.append((x["ops_per_s"], x["ms"]["p50"], x["ms"]["p99"], (st.get("flush_ms", 0) / st["flushes"]) if st.get("flushes") else None, st.get("checkpoints")))
    if rs:
        ops = [r[0] for r in rs]; p50 = [r[1] for r in rs]
        print(f"SUMMARY {arm}: runs {len(rs)} ops/s median {statistics.median(ops)} range {min(ops)}-{max(ops)}; p50 ms median {statistics.median(p50)} range {min(p50)}-{max(p50)}; flush ms/flush {[round(r[3], 2) if r[3] else None for r in rs]}; checkpoints {[r[4] for r in rs]}")
PY
echo "$(date -u +%H:%M:%SZ) clippy $(state)"
nice -n 10 cargo clippy --offline --manifest-path bench/Cargo.toml --bin amphora-bench --all-targets -- -D warnings >"$O/clippy.log" 2>&1; echo "clippy rc=$?"; tail -5 "$O/clippy.log"
git checkout -q --detach "$A"
echo "$(date -u +%H:%M:%SZ) done $(state)"
