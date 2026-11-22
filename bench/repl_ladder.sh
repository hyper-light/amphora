#!/usr/bin/env bash
# Commit latency on the concurrency ladder, every system on this one host, natively, through one
# client (CP-T1.8, AC-T1.8.2; Ada's rules of 2026-11-22: same client and same platform for every
# system, containers for all or for none; the ladder 1, 8, 32, 128, 512, 2048, 4096). For each rung
# and each system it brings a fresh three-member cluster up, runs `amphora-bench --repl commit
# --writers W`, and takes it down: Amphora's members as processes on 127.0.0.1 (repl_members.sh),
# the competitors as processes from servers/repl/native (run native/setup.sh on this host first).
# Every record names the client, the platform, the system's version and what its acknowledgement
# means (`about`, `platform`), and the client's own CPU, memory and sockets (`client_use`), so a rung
# where the client was the bottleneck shows.
#
# usage: bench/repl_ladder.sh RECORDS [FLAG VALUE ...]
#   FLAGs: amphora-bench --repl commit's, every size and duration (none picked here):
#          --batch --text-bytes --deadline-s --warmup-s --seconds, and Amphora's --relay-records and
#          (for the replicated class) --spanning. --writers is the ladder's.
# Environment:
#   SYSTEMS  default "amphora-procs-persisted amphora-procs-replicated rqlite dqlite libsql litefs".
#            amphora-procs-persisted is the class whose acknowledgement matches rqlite's and dqlite's
#            (a quorum's disks); libSQL and LiteFS acknowledge on the primary alone (their `about`).
#   LADDER   default "1 8 32 128 512 2048 4096" (tiny instances may stop at 128).
#   PROCS_PORT  Amphora's first member's port (default 54290; members take it and the next two).
# Records: RECORDS/<system>-commit.jsonl (one line a rung), RECORDS/host.json (this host), and the
# table printed at the end (p50, p99, p99.9 in ms, commits a second, the client's cores in use).
# Exit: 0 when every rung of every system gave a record or a typed refusal; 1 otherwise.
set -uo pipefail
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
RECORDS=${1:?usage: bench/repl_ladder.sh RECORDS [FLAG VALUE ...]}
shift
FLAGS=("$@")
SYSTEMS=${SYSTEMS:-amphora-procs-persisted amphora-procs-replicated rqlite dqlite libsql litefs}
LADDER=${LADDER:-1 8 32 128 512 2048 4096}
PROCS_PORT=${PROCS_PORT:-54290}
NATIVE="$HERE/servers/repl/native"
mkdir -p "$RECORDS"
# The top rung holds a connection a writer to each node it reaches: take the host's hard limit.
ulimit -n "$(ulimit -Hn)" 2>/dev/null || true
python3 -I -c 'import json, os, platform, sys
mem = next((l.split()[1] for l in open("/proc/meminfo") if l.startswith("MemTotal:")), None)
bins = os.path.expanduser(os.environ.get("AMPHORA_REPL_BIN", "~/.cache/amphora-repl-bin"))
v = open(os.path.join(bins, "versions.txt")).read().split("\n") if os.path.exists(os.path.join(bins, "versions.txt")) else []
print(json.dumps({"host": platform.node(), "kernel": platform.release(), "machine": platform.machine(), "cpus": os.cpu_count(),
  "mem_kib": mem, "data_fs": sys.argv[1], "open_files": sys.argv[2], "fetched": [l for l in v if l], "ladder": sys.argv[3]}))' \
  "$(stat -f -c %T "$HOME/.cache" 2>/dev/null || echo unknown)" "$(ulimit -n)" "$LADDER" >"$RECORDS/host.json"
cat "$RECORDS/host.json" >&2
cargo build --release --manifest-path "$HERE/Cargo.toml" || exit 1
TARGET=$(cargo metadata --format-version 1 --no-deps --manifest-path "$HERE/Cargo.toml" | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
BIN="$TARGET/release/amphora-bench"
[[ -x "$BIN" ]] || { echo "no $BIN" >&2; exit 1; }
MEMBERS="127.0.0.1:$PROCS_PORT 127.0.0.1:$((PROCS_PORT + 1)) 127.0.0.1:$((PROCS_PORT + 2))"

failed=0
for w in $LADDER; do
  for s in $SYSTEMS; do
    echo "== $s writers $w" >&2
    case "$s" in
      amphora-procs-*)
        # shellcheck disable=SC2086
        MEASURES=commit "$HERE/repl_members.sh" "$RECORDS" "${s#amphora-procs-}" $MEMBERS "${FLAGS[@]}" --writers "$w" || failed=1
        ;;
      *)
        out="$RECORDS/$s-commit.jsonl"
        if "$BIN" --repl commit --system "$s" --scripts "$NATIVE" --record "$out" "${FLAGS[@]}" --writers "$w"; then
          echo "ok $s $w" >&2
        elif tail -n 1 "$out" 2>/dev/null | grep -q '"refused"'; then
          echo "refused $s $w: $(tail -n 1 "$out" | sed -n 's/.*"why":"\([^"]*\)".*/\1/p')" >&2
        else
          echo "FAILED $s $w" >&2
          failed=1
        fi
        ;;
    esac
  done
done

python3 -I -c 'import glob, json, os, sys
rows = []
for f in sorted(glob.glob(os.path.join(sys.argv[1], "*-commit.jsonl"))):
    for line in open(f):
        r = json.loads(line)
        a, u, l = r.get("about") or {}, r.get("client_use") or {}, r.get("latency_ms") or {}
        wall = (r.get("warmup_s") or 0) + (r.get("seconds") or 0)
        cores = (u.get("cpu_s") or 0) / wall if wall else None
        rows.append((r.get("system"), r.get("writers"), l.get("p50"), l.get("p99"), l.get("p999"), r.get("writes_per_s"), cores, u.get("sockets"), a.get("platform"), a.get("version"), r.get("refused")))
print("system\twriters\tp50_ms\tp99_ms\tp99.9_ms\tcommits_s\tclient_cores\tsockets\tplatform\tversion\trefused")
for row in sorted(rows, key=lambda r: (str(r[0]), r[1] or 0)):
    print("\t".join("" if v is None else (f"{v:.3f}" if isinstance(v, float) else str(v)) for v in row))' "$RECORDS"
exit $failed
