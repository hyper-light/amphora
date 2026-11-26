#!/usr/bin/env bash
# Comprehensive multi-scale SQLite + Postgres suite runner for Amphora.
#
# Exercises all 89 operations in `bench --suite` (simple point/range reads, challenging
# analytical queries, high-cardinality aggregates, wide unindexed sorts, skewed multi-way
# joins, recursive CTEs, window frames, compound subqueries, ordered-set aggregates,
# Round 1-9 regression workloads, inserts, updates, deletes, savepoint rollbacks,
# mixed transactions, catalog introspection, and DDL/view/temp-table lifecycle) across:
#   - Small dataset  (5,000 items: fits in L2/L3 & per-connection cache; exposes CPU/VM overhead)
#   - Medium dataset (200,000 items: ~130 MB; exceeds private 2,000-page cache, stresses SharedPages & planner)
#   - Large/Spilling (1,000,000 items, or custom LARGE_ITEMS, with constrained reader cache)
# and across both SQLite (`amphora`, `sqlite`) and Postgres (`amphora-pg`, optional `postgres` via PG_DSN),
# verifying every read answer against SQLite.
#
# Usage:
#   bash bench/comprehensive.sh [OUT_DIR]
# Env overrides:
#   BENCH_BIN       path to release amphora-bench binary
#   SCALES          space-separated scales to run (default: "small medium") — add "large" for 1M+ spilling pass
#   SMALL_ITEMS     default 5000
#   MEDIUM_ITEMS    default 200000
#   LARGE_ITEMS     default 1000000
#   CLIENTS         default "1,32"
#   RUN_SECONDS     default 2
#   WARMUP          default 0.5
#   ONLY            optional --only filter
#   PG_DSN          optional external PostgreSQL DSN (e.g. postgres://postgres@127.0.0.1:55432/postgres)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-${HOME}/.cache/amphora-comprehensive-$(date +%Y%m%d-%H%M%S)}"
mkdir -p "$OUT"

BENCH_BIN_OVERRIDE="${BENCH_BIN:-}"
BENCH_BIN="${BENCH_BIN_OVERRIDE:-$ROOT/bench/target/release/amphora-bench}"
HEAD_COMMIT="$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)"
if [[ -z "$BENCH_BIN_OVERRIDE" && "${SKIP_BUILD:-0}" != "1" ]]; then
  echo "Building amphora-bench release binary (HEAD=${HEAD_COMMIT:0:12})..." >&2
  (cd "$ROOT/bench" && CARGO_INCREMENTAL=0 cargo build --release)
elif [[ ! -x "$BENCH_BIN" ]]; then
  echo "ERROR: BENCH_BIN=$BENCH_BIN is missing or not executable" >&2
  exit 1
fi

SCALES="${SCALES:-small medium}"
SMALL_ITEMS="${SMALL_ITEMS:-5000}"
MEDIUM_ITEMS="${MEDIUM_ITEMS:-200000}"
LARGE_ITEMS="${LARGE_ITEMS:-1000000}"
CLIENTS="${CLIENTS:-1,32}"
RUN_SECONDS="${RUN_SECONDS:-2}"
WARMUP="${WARMUP:-0.5}"
ONLY_ARGS=()
if [[ -n "${ONLY:-}" ]]; then
  ONLY_ARGS=(--only "$ONLY")
fi

TARGETS=(amphora amphora-pg sqlite)
if [[ -n "${PG_DSN:-}" ]]; then
  TARGETS+=(postgres)
fi

for scale in $SCALES; do
  case "$scale" in
    small)
      items="$SMALL_ITEMS"
      extra_env=()
      ;;
    medium)
      items="$MEDIUM_ITEMS"
      extra_env=()
      ;;
    large)
      items="$LARGE_ITEMS"
      # Constrain reader cache so large sorts/scans/joins exercise real cache pressure & spilling.
      extra_env=(AMPHORA_READER_PAGES=512)
      ;;
    *)
      echo "Unknown scale: $scale (expected small, medium, or large)" >&2
      exit 2
      ;;
  esac

  echo "==================================================================" >&2
  echo "Scale: $scale (items=$items, clients=$CLIENTS, targets=${TARGETS[*]})" >&2
  echo "==================================================================" >&2

  jsonl="$OUT/${scale}.jsonl"
  : > "$jsonl"

  for target in "${TARGETS[@]}"; do
    dbdir="$OUT/db-${scale}-${target}"
    rm -rf "$dbdir"
    args=(--suite --target "$target" --dir "$dbdir" --items "$items" --clients "$CLIENTS" --warmup "$WARMUP" --seconds "$RUN_SECONDS")
    if [[ "$target" == "postgres" ]]; then
      args+=(--dsn "$PG_DSN")
    fi
    if [[ ${#ONLY_ARGS[@]} -gt 0 ]]; then
      args+=("${ONLY_ARGS[@]}")
    fi
    rec="$OUT/${scale}-${target}.json"
    echo ">>> Running target=$target scale=$scale items=$items" >&2
    if [[ ${#extra_env[@]} -gt 0 ]]; then
      env "${extra_env[@]}" "$BENCH_BIN" "${args[@]}" > "$rec"
    else
      "$BENCH_BIN" "${args[@]}" > "$rec"
    fi
    cat "$rec" >> "$jsonl"
    rm -rf "$dbdir"
  done

  echo ">>> Verifying answers and build revision for scale=$scale..." >&2
  VERIFY_ARGS=("$jsonl" sqlite)
  if [[ -n "${PG_DSN:-}" ]]; then
    VERIFY_ARGS+=(--require-pg-ref)
  fi
  python3 "$ROOT/bench/suite_answers.py" "${VERIFY_ARGS[@]}"

  python3 - "$jsonl" "$scale" "$HEAD_COMMIT" "${ALLOW_STALE_BIN:-0}" <<'PY'
import json, sys
path, scale, head_commit, allow_stale = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4] == "1"
records = {}
for line in open(path):
    if not line.strip():
        continue
    d = json.loads(line)
    records[d["target"]] = d

targets = sorted(records)
print(f"\n=== SUMMARY ({scale}) ===")
for t in targets:
    d = records[t]
    commit = d.get("build_commit", "missing")
    dirty = d.get("build_dirty", 0)
    if not allow_stale and head_commit != "unknown" and commit != head_commit:
        raise SystemExit(
            f"ERROR: target {t} ran stale binary commit={commit} (expected HEAD={head_commit}); "
            "rebuild amphora-bench or set ALLOW_STALE_BIN=1"
        )
    errs = sum(r.get("errors", 0) for r in d["results"])
    unsup = sum(1 for r in d["results"] if r.get("unsupported"))
    print(f"  {t:12s}: commit={commit[:12]} dirty={dirty} load={d['load_s']:.2f}s index={d['index_s']:.2f}s settle={d['settle_s']:.2f}s ops={len(d['results'])} errors={errs} unsupported={unsup}")
PY
done

echo "All comprehensive suite results saved in $OUT" >&2
