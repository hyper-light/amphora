#!/usr/bin/env bash
# Amphora's three members each its own process, on the machines given as arguments (CP-T1.8; the
# LAN p99 the in-process `amphora` system can't give): for each measure, starts a member at each
# address (`amphora-bench --repl-member`, its data folder emptied first), runs
# `amphora-bench --repl MEASURE --system amphora-procs` against the three, which stops them as it
# ends, and appends its record (or its typed refusal) to RECORDS/amphora-procs-CLASS-MEASURE.jsonl.
# Nothing here picks a machine or a port: the person running it names them.
#
# usage: bench/repl_members.sh RECORDS CLASS HOST1:PORT HOST2:PORT HOST3:PORT [FLAG VALUE ...]
#   CLASS       persisted or replicated (replicated takes --spanning among the FLAGs).
#   HOSTn:PORT  each member's address as its peers reach it; it listens there. A host that is this
#               machine (localhost, 127.0.0.1 or `hostname`) runs its member here, so three on one
#               machine are three ports of localhost. Any other is reached by ssh and needs a clone
#               of this repository at LAB_REPO (default this checkout's path relative to its home),
#               at this commit, and cargo; the member is built there.
#   FLAGs       amphora-bench --repl's (every size and duration; none picked here), --deadline-s
#               and --relay-records among them, which the members take too, with --spanning.
# Environment: MEASURES (default "commit failover catchup"); MEMBER_DIR, the members' data folder
#   on each machine (default ~/.cache/amphora-repl-members), one folder a member under it.
# Exit: 0 when every measure gave a record or a typed refusal; 1 otherwise.
set -uo pipefail
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd -- "$HERE/.." && pwd)"
[[ $# -ge 5 ]] || { echo "usage: $0 RECORDS CLASS HOST1:PORT HOST2:PORT HOST3:PORT [FLAG VALUE ...]" >&2; exit 2; }
RECORDS=$1 CLASS=$2 ADDRS=("$3" "$4" "$5")
shift 5
FLAGS=("$@")
MEASURES=${MEASURES:-commit failover catchup}
MEMBER_DIR=${MEMBER_DIR:-.cache/amphora-repl-members}
LAB_REPO=${LAB_REPO:-${ROOT#"$HOME"/}}
PEERS=$(IFS=,; echo "${ADDRS[*]}")
q() { printf '%q ' "$@"; }

# The members take three of the FLAGs: their deadline, their relay depth and the class's span.
MEMBER_FLAGS=(--class "$CLASS")
for ((i = 0; i < ${#FLAGS[@]}; i++)); do
  case "${FLAGS[$i]}" in
    --deadline-s | --relay-records | --spanning) MEMBER_FLAGS+=("${FLAGS[$i]}" "${FLAGS[$((i + 1))]:-}") ;;
  esac
done

mkdir -p "$RECORDS"
cargo build --release --manifest-path "$HERE/Cargo.toml" || exit 1
TARGET=$(cargo metadata --format-version 1 --no-deps --manifest-path "$HERE/Cargo.toml" | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
BIN="$TARGET/release/amphora-bench"
[[ -x "$BIN" || -x "$BIN.exe" ]] || { echo "no $BIN" >&2; exit 1; }
here() { [[ "$1" == localhost || "$1" == 127.0.0.1 || "$1" == "$(hostname)" ]]; }

failed=0
for m in $MEASURES; do
  pids=()
  for i in 0 1 2; do
    addr=${ADDRS[$i]} host=${ADDRS[$i]%:*} node=$((i + 1))
    args=(--repl-member --node "$node" --peers "$PEERS" --dir "$HOME/$MEMBER_DIR/$node" "${MEMBER_FLAGS[@]}")
    if here "$host"; then
      "$BIN" "${args[@]}" &
    else
      ssh "$host" "cd $(q "$LAB_REPO") && cargo run -q --release --manifest-path bench/Cargo.toml -- --repl-member --node $node --peers $(q "$PEERS") --dir \"\$HOME\"/$(q "$MEMBER_DIR/$node") $(q "${MEMBER_FLAGS[@]}")" &
    fi
    pids+=($!)
  done
  out="$RECORDS/amphora-procs-$CLASS-$m.jsonl"
  echo "== amphora-procs-$CLASS $m on $PEERS" >&2
  # The driver waits for the members as it waits for any node: each request retried until --deadline-s.
  if "$BIN" --repl "$m" --system amphora-procs --class "$CLASS" --members "$PEERS" --record "$out" "${FLAGS[@]}"; then
    echo "ok amphora-procs-$CLASS $m" >&2
  elif tail -n 1 "$out" 2>/dev/null | grep -q '"refused"'; then
    echo "refused amphora-procs-$CLASS $m" >&2
  else
    echo "FAILED amphora-procs-$CLASS $m" >&2
    failed=1
  fi
  for p in "${pids[@]}"; do wait "$p" || { echo "a member of $m exited non-zero" >&2; failed=1; }; done
done
exit $failed
