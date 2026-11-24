#!/usr/bin/env bash
# Regenerates answers.txt in crates/amphora/tests/extended, copy and catalog: what a PostgreSQL 19 server answers the
# messages of each one's steps.txt (scripts/postgres-extended-answers.py), from a running 19
# server that asks for no password.
#
# usage: scripts/postgres-extended-answers.sh <host> <port>
set -euo pipefail
HOST=${1:?host} PORT=${2:?port}
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../crates/amphora/tests"
for list in extended copy catalog; do
  python3 ../../../scripts/postgres-extended-answers.py "$HOST" "$PORT" "$list/steps.txt" > "$list/answers.txt"
done
