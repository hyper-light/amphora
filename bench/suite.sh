#!/usr/bin/env bash
# The comprehensive suite (src/suite.rs) on every target, each through run.sh (its server restarted,
# its memory sampled, the host, commit and load recorded), records appended to $SUITE_OUT
# (default records/<date>-<host>-suite.jsonl).
#
# usage: bench/suite.sh [targets] [suite arguments...]
#   e.g. bench/suite.sh amphora,postgres --items 200000 --clients 1,32 --seconds 5 --warmup 1
set -uo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")"
targets="${1:-amphora,turso,sqlite,postgres,orioledb,cedardb,mysql,clickhouse}"
shift || true
out="${SUITE_OUT:-records/$(date -u +%Y-%m-%d)-$(hostname -s)-suite.jsonl}"
mkdir -p "$(dirname -- "$out")"
err="${XDG_CACHE_HOME:-$HOME/.cache}/amphora-suite-$$.err"
for t in ${targets//,/ }; do
  echo "== $t load=$(cut -d' ' -f1 /proc/loadavg) $(date +%T)" >&2
  if line=$(./run.sh "$t" --suite "$@" 2>"$err"); then
    echo "$line" >> "$out"
  else
    python3 - "$t" "$(tail -c 2000 "$err")" >> "$out" <<'PY'
import json, sys
print(json.dumps({"failed": True, "target": sys.argv[1], "error": sys.argv[2]}))
PY
  fi
  rm -f "${err:?}"
done
echo "records: $out" >&2
