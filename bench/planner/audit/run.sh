#!/usr/bin/env bash
# The planner audit's demonstrations (docs/research/planner.md, "The audit"), on a release build of
# amphora-bench: each NN-*.sql through `amphora-bench --eqp` into NN-*.out, every recorded CERT
# finding replayed into cert-replay.out, and the planner's panicking calls counted into panics.out.
# With --check it writes nothing and compares each output with the one kept, the lines holding a
# time (`prepare:`) left out, and exits 1 on any difference.
#
#   bench/planner/audit/run.sh BENCH_BINARY [--check]
set -euo pipefail
here="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
bin="${1:?the amphora-bench binary}"
check="${2:-}"
scratch="${AUDIT_SCRATCH:-$HOME/.cache/planner-audit}"
mkdir -p "$scratch"
differ=0
keep() {
  local name="$1" got="$2"
  if [ "$check" = "--check" ]; then
    if ! diff <(grep -v '^prepare: ' "$here/$name") <(grep -v '^prepare: ' "$got") > "$scratch/$name.diff"; then
      echo "differs: $name ($scratch/$name.diff)"
      differ=1
    fi
  else
    cp "$got" "$here/$name"
  fi
}
for sql in "$here"/[0-9][0-9]-*.sql; do
  name="$(basename "$sql" .sql).out"
  "$bin" --eqp --script "$sql" --dir "$scratch/db" > "$scratch/$name"
  keep "$name" "$scratch/$name"
done
python3 -I "$here/cert_replay.py" "$bin" "$here/../../records/2026-10-24-adacachy-oracles" "$scratch/cert" > "$scratch/cert-replay.out"
keep cert-replay.out "$scratch/cert-replay.out"
exit "$differ"
