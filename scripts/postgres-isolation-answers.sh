#!/usr/bin/env bash
# Regenerates crates/amphora/tests/isolation/answers.txt: what a PostgreSQL 19 server answers the steps of
# steps.txt (scripts/postgres-isolation-answers.py), from a running 19 server that asks for no password.
#
# usage: scripts/postgres-isolation-answers.sh <host> <port>
set -euo pipefail
HOST=${1:?host} PORT=${2:?port}
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../crates/amphora/tests/isolation"
python3 ../../../../scripts/postgres-isolation-answers.py "$HOST" "$PORT" steps.txt > answers.txt
