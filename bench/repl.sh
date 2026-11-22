#!/usr/bin/env bash
# Replication measured (CP-T1.8; G12 steps 1 and 3-5): every measure on every system with one
# command. Builds amphora-bench, then runs `amphora-bench --repl MEASURE --system SYSTEM` for each
# pair, appending each record (or its typed refusal) to RECORDS/<system>-<measure>.jsonl.
#
# usage: bench/repl.sh RECORDS [FLAG VALUE ...]
#   FLAGs are amphora-bench's (bench/src/repl.rs): every size and duration, none picked here. Pass
#   every flag the chosen measures take; each run is given all of them.
#   MEASURES  (env, default "commit failover catchup")
#   SYSTEMS   (env, default "amphora-persisted amphora-replicated rqlite dqlite litefs libsql")
#   amphora-replicated also needs --spanning; amphora needs --dir and --relay-records.
# Exit: 0 when every pair gave a record or a typed refusal; 1 when any failed otherwise.
set -uo pipefail
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
RECORDS=${1:?usage: bench/repl.sh RECORDS [FLAG VALUE ...]}
shift
MEASURES=${MEASURES:-commit failover catchup}
SYSTEMS=${SYSTEMS:-amphora-persisted amphora-replicated rqlite dqlite litefs libsql}
mkdir -p "$RECORDS"
cargo build --release --manifest-path "$HERE/Cargo.toml" || exit 1
TARGET=$(cargo metadata --format-version 1 --no-deps --manifest-path "$HERE/Cargo.toml" | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
BIN="$TARGET/release/amphora-bench"
[[ -x "$BIN" || -x "$BIN.exe" ]] || { echo "no $BIN" >&2; exit 1; }
# On Windows the binary resolves a bare `bash` to WSL's: hand it this bash by its Windows path.
SHELL_PATH=$(command -v bash)
command -v cygpath >/dev/null && SHELL_PATH=$(cygpath -w "$SHELL_PATH")
failed=0
for m in $MEASURES; do
  for s in $SYSTEMS; do
    case "$s" in
      amphora-*) sys=(--system amphora --class "${s#amphora-}") ;;
      *) sys=(--system "$s") ;;
    esac
    out="$RECORDS/$s-$m.jsonl"
    echo "== $s $m" >&2
    before=$(wc -l <"$out" 2>/dev/null || echo 0)
    if "$BIN" --repl "$m" "${sys[@]}" --shell "$SHELL_PATH" --record "$out" "$@"; then
      echo "ok $s $m" >&2
    elif [[ $(wc -l <"$out" 2>/dev/null || echo 0) -gt $before ]] && tail -n 1 "$out" | grep -q '"refused"'; then
      echo "refused $s $m: $(tail -n 1 "$out" | sed -n 's/.*"why":"\([^"]*\)".*/\1/p')" >&2
    else
      echo "FAILED $s $m" >&2
      failed=1
    fi
  done
done
exit $failed
