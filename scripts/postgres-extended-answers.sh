#!/usr/bin/env bash
# Regenerates crates/amphora/tests/extended/answers.txt: what a PostgreSQL 19 server answers the messages of steps.txt
# (scripts/postgres-extended-answers.py), from a running 19 server that asks for no password.
#
# usage: scripts/postgres-extended-answers.sh <host> <port>
set -euo pipefail
HOST=${1:?host} PORT=${2:?port}
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../crates/amphora/tests/extended"
python3 ../../../../scripts/postgres-extended-answers.py "$HOST" "$PORT" steps.txt > answers.txt
