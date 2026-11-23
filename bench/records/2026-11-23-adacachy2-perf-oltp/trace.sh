#!/usr/bin/env bash
# Lane W2 (oltp): where one insert's time goes, Amphora (B = e57a028b, the suite's engine reports
# uncopied) beside SQLite, insert_one alone at 1 client. stages.c timestamps the log's writes, its
# open, flush and close, and the futex hand-offs; stages.py lays them out per insert. 3 rounds, each
# Amphora plain, Amphora traced, SQLite plain, SQLite traced (the plain runs show the shim's cost).
# Then the branch's bench builds and clippy reports anything in the lines this lane changed.
set -uo pipefail
SHA=${1:?branch sha}
L=$HOME/.cache/amphora-perf/oltp-run; R=$HOME/.cache/amphora-perf/oltp-remote; BIN=$HOME/.cache/amphora-perf-bin/oltp
O=$L/trace-w2; mkdir -p "$O"; rm -f "$O"/*
state() { echo "load $(cut -d' ' -f1-3 /proc/loadavg) cpu_psi $(head -1 /proc/pressure/cpu | cut -d' ' -f2) io_psi $(head -1 /proc/pressure/io | cut -d' ' -f2)"; }
cd "$R" || exit 3
git fetch -q origin perf/oltp && git checkout -q --detach "$SHA" || { echo "checkout failed"; exit 3; }
S=bench/records/2026-11-23-adacachy2-perf-oltp
cc -O2 -shared -fPIC -o "$O/stages.so" "$S/stages.c" -ldl || exit 4
D=$L/data-ab; mkdir -p "$D" && chattr +C "$D" 2>/dev/null
export XDG_CACHE_HOME=$L/xdg-ab
EXE=$BIN/amphora-bench-B-e57a028b
ARGS=(--suite --only 'insert_one$' --items 200000 --clients 1 --seconds 1 --warmup 0.5)
one() {  # target label traced
  rm -rf "${D:?}/x"; local s; s=$(state)
  if [ "$3" = 1 ]; then
    STAGES_OUT="$O/$2.rec" LD_PRELOAD="$O/stages.so" nice -n 10 "$EXE" --target "$1" --dir "$D/x" "${ARGS[@]}" 2>>"$O/runs.err" | tail -1 >"$O/$2.json"
  else
    nice -n 10 "$EXE" --target "$1" --dir "$D/x" "${ARGS[@]}" 2>>"$O/runs.err" | tail -1 >"$O/$2.json"
  fi
  python3 -I -c 'import json,sys; r=json.load(open(sys.argv[1])); x=r["results"][0]; st=r.get("stats") or {}
print(sys.argv[2], "ops/s", x["ops_per_s"], "p50", x["ms"]["p50"], "p90", x["ms"]["p90"], "p99", x["ms"]["p99"], "n", x["ms"]["n"], "errors", x["errors"], "commits", st.get("commits", "-"), "flushes", st.get("flushes", "-"), "checkpoints", st.get("checkpoints", "-"), sys.argv[3])' "$O/$2.json" "$2" "$s"
}
for i in 1 2 3; do
  one amphora "r$i-amphora-plain" 0; one amphora "r$i-amphora-traced" 1
  one sqlite "r$i-sqlite-plain" 0; one sqlite "r$i-sqlite-traced" 1
done
for f in "$O"/r*-traced.rec; do
  side=amphora; case $f in *sqlite*) side=sqlite;; esac
  echo "== $(basename "$f")"; python3 -I "$S/stages.py" "$f" "$side"
done
export CARGO_HOME=$HOME/.cache/cargo-home-perf-oltp CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/oltp CARGO_BUILD_JOBS=8 CARGO_INCREMENTAL=0 \
  CARGO_PROFILE_RELEASE_LTO=thin CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS=true CARGO_PROFILE_RELEASE_DEBUG=line-tables-only
echo "$(date -u +%H:%M:%SZ) build $(state)"
nice -n 10 cargo build --release --offline --manifest-path bench/Cargo.toml --bin amphora-bench >"$O/build.log" 2>&1; echo "build rc=$?"; tail -3 "$O/build.log"
nice -n 10 cargo clippy --offline --manifest-path bench/Cargo.toml --bin amphora-bench --all-targets -- -D warnings >"$O/clippy.log" 2>&1; echo "clippy rc=$?"
echo "clippy errors at store.rs or suite_db.rs lines this lane changed:"; grep -n -E -- '--> src/(store\.rs:1(3[0-9]|4[0-9])|suite_db\.rs:2(2[0-9]|3[0-9])):' "$O/clippy.log" || echo "none"
grep -c '^error' "$O/clippy.log"
echo "$(date -u +%H:%M:%SZ) done $(state)"
