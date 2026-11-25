#!/usr/bin/env bash
# Meta-Scale Agentic End-User Gauntlet runner across Amphora (SQLite), Amphora (Postgres),
# reference SQLite, and optional external PostgreSQL.
#
# Scale presets (~19.5 rows per scale unit across 43 interconnected tables):
#   empty    -> 0 rows (boundary sweep)
#   single   -> ~45 rows (1-row boundary sweep)
#   smoke    -> ~500K rows (quick differential verification)
#   millions -> ~3M rows   (default dev/local)
#   tens     -> ~30M rows  (production warm)
#   sweat    -> ~117M+ rows (hundreds of millions, single-node endurance)
#   meta     -> ~500M rows per shard (multi-billion with SHARDS=8..64!)
#
# Usage:
#   SCALE=millions SHARDS=1 LOGICAL_AGENTS=1000000000 CHAOS_WORKERS=64 bash bench/gauntlet.sh
#   SCALE=sweat SHARDS=4 LOGICAL_AGENTS=5000000000 CHAOS_WORKERS=128 TARGETS="amphora amphora-pg" bash bench/gauntlet.sh

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT_DIR="${OUT_DIR:-$HOME/.cache/amphora-gauntlet-run}"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/bench/target}"
SCALE="${SCALE:-millions}"
SHARDS="${SHARDS:-1}"
LOGICAL_AGENTS="${LOGICAL_AGENTS:-1000000000}"
CHAOS_WORKERS="${CHAOS_WORKERS:-64}"
SECONDS_OP="${SECONDS_OP:-0.25}"
CHAOS_SECONDS="${CHAOS_SECONDS:-5}"
PHASES="${PHASES:-all}"
TARGETS="${TARGETS:-sqlite amphora amphora-pg}"

mkdir -p "$OUT_DIR"

echo "==> Building amphora-bench (release) into $TARGET_DIR..."
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR="$TARGET_DIR" \
  cargo build --release --manifest-path "$ROOT/bench/Cargo.toml"

BIN="$TARGET_DIR/release/amphora-bench"
REPORTS=()
OVERALL_EXIT=0

for T in $TARGETS; do
  echo ""
  echo "==> Running --gauntlet target=$T scale=$SCALE shards=$SHARDS logical_agents=$LOGICAL_AGENTS workers=$CHAOS_WORKERS"
  DB_DIR="$OUT_DIR/db-$T-$SCALE"
  JSON_OUT="$OUT_DIR/$SCALE-$T.json"
  rm -rf "$DB_DIR"
  EXTRA=()
  if [[ "$T" == "postgres" && -n "${PG_DSN:-}" ]]; then
    EXTRA+=(--dsn "$PG_DSN")
  fi
  if ! "$BIN" --gauntlet \
    --target "$T" \
    --dir "$DB_DIR" \
    --scale "$SCALE" \
    --shards "$SHARDS" \
    --logical-agents "$LOGICAL_AGENTS" \
    --chaos-workers "$CHAOS_WORKERS" \
    --seconds "$SECONDS_OP" \
    --chaos-seconds "$CHAOS_SECONDS" \
    --phases "$PHASES" \
    --answers \
    ${EXTRA[@]+"${EXTRA[@]}"} \
    > "$JSON_OUT"; then
    OVERALL_EXIT=1
  fi
  rm -rf "$DB_DIR"
  REPORTS+=("$JSON_OUT")
done

if [[ "${#REPORTS[@]}" -ge 2 ]]; then
  if ! python3 "$ROOT/bench/gauntlet_diff.py" "${REPORTS[@]}"; then
    OVERALL_EXIT=1
  fi
fi

exit "$OVERALL_EXIT"
