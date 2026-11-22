#!/usr/bin/env bash
# Scale-out and regions measured (CP-T1.9 to CP-T1.11; G13 steps 1 and 5, G14 steps 1 and 2):
# every measure on every system with one command. Builds amphora-bench, then runs
# `amphora-bench --scale MEASURE --system SYSTEM` for each pair, appending each record (or its typed
# refusal) to RECORDS/<system>-<measure>.jsonl.
#
# usage: bench/scale.sh RECORDS [FLAG VALUE ...]
#   FLAGs are amphora-bench's (bench/src/scale.rs): every size and duration, none picked here. Pass
#   every flag the chosen measures take; each run is given all of them.
#   MEASURES  (env, default "write read follower xgroup region regionloss")
#   SYSTEMS   (env, default "amphora cockroach tidb")
#   amphora also takes --dir, --class, --relay-records and --relay-pending (and --spanning,
#   --protocol); the others ignore them.
#   KEEP_IMAGES=1 keeps the images the containers ran; otherwise they're removed by ID at the end.
# Exit: 0 when every pair gave a record or a typed refusal; 1 when any failed otherwise.
set -uo pipefail
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
RECORDS=${1:?usage: bench/scale.sh RECORDS [FLAG VALUE ...]}
shift
MEASURES=${MEASURES:-write read follower xgroup region regionloss}
SYSTEMS=${SYSTEMS:-amphora cockroach tidb}
mkdir -p "$RECORDS"
cargo build --release --manifest-path "$HERE/Cargo.toml" || exit 1
TARGET=$(cargo metadata --format-version 1 --no-deps --manifest-path "$HERE/Cargo.toml" | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
BIN="$TARGET/release/amphora-bench"
[[ -x "$BIN" || -x "$BIN.exe" ]] || { echo "no $BIN" >&2; exit 1; }
# On Windows the binary resolves a bare `bash` to WSL's: hand it this bash by its Windows path.
SHELL_PATH=$(command -v bash)
command -v cygpath >/dev/null && SHELL_PATH=$(cygpath -w "$SHELL_PATH")
# Flags only Amphora takes are dropped for the others.
others=()
skip=0
for a in "$@"; do
  if ((skip)); then skip=0; continue; fi
  case "$a" in
    --dir|--class|--spanning|--relay-records|--relay-pending|--protocol) skip=1 ;;
    *) others+=("$a") ;;
  esac
done
failed=0
for m in $MEASURES; do
  for s in $SYSTEMS; do
    out="$RECORDS/$s-$m.jsonl"
    echo "== $s $m" >&2
    if [[ $s == amphora ]]; then flags=("$@"); else flags=("${others[@]}"); fi
    before=$(wc -l <"$out" 2>/dev/null || echo 0)
    if "$BIN" --scale "$m" --system "$s" --shell "$SHELL_PATH" --record "$out" "${flags[@]}"; then
      echo "ok $s $m" >&2
    elif [[ $(wc -l <"$out" 2>/dev/null || echo 0) -gt $before ]] && tail -n 1 "$out" | grep -q '"refused"'; then
      echo "refused $s $m: $(tail -n 1 "$out" | sed -n 's/.*"why":"\([^"]*\)".*/\1/p')" >&2
    else
      echo "FAILED $s $m" >&2
      failed=1
    fi
  done
done
if [[ ${KEEP_IMAGES:-0} != 1 ]]; then
  for s in $SYSTEMS; do
    [[ -f "$HERE/servers/scale/$s.sh" ]] || continue
    for id in $(WAIT_S=1 bash "$HERE/servers/scale/$s.sh" images); do
      docker image rm "$id" >/dev/null 2>&1 || true
    done
  done
fi
exit $failed
