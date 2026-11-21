#!/usr/bin/env bash
# AC-G3.5.2 (decision 41): point_pk at one client with each descent-skip setting, the same database
# reused, records appended to $SUITE_OUT (default records/<date>-<host>-descent-skip.jsonl). The winner
# passes at 20% more queries a second than `off`. A measurement: the coordinator's, on a quiet host
# (CLAUDE.md §7), after `cargo build --release --locked -p amphora-bench`.
#
# usage: bench/descent_skip.sh [suite arguments...]
#   e.g. bench/descent_skip.sh --items 1000000 --seconds 20 --warmup 5
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
out="${SUITE_OUT:-records/$(date -u +%Y-%m-%d)-$(hostname -s)-descent-skip.jsonl}"
mkdir -p "$(dirname -- "$out")"
reuse=()
for setting in off hash locator off hash locator; do
  echo "== AMPHORA_DESCENT_SKIP=$setting load=$(cut -d' ' -f1 /proc/loadavg 2>/dev/null || echo ?) $(date +%T)" >&2
  line=$(AMPHORA_DESCENT_SKIP="$setting" ./run.sh amphora --suite --only 'point_pk$' --clients 1 "${reuse[@]}" "$@")
  python3 - "$setting" "$line" >> "$out" <<'PY'
import json, sys
record = json.loads(sys.argv[2])
record["descent_skip"] = sys.argv[1]
print(json.dumps(record))
PY
  reuse=(--reuse)
done
echo "records: $out" >&2
