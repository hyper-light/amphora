#!/usr/bin/env bash
# Regenerates crates/amphora/tests/tpcb/{answers,listener-answers}.txt: what a PostgreSQL 19 server answers
# statements.txt and listener.txt, message by message (scripts/postgres-tpcb-answers.py), from a running 19 server
# that asks for no password.
#
# usage: scripts/postgres-tpcb-answers.sh <host> <port>
set -euo pipefail
HOST=${1:?host} PORT=${2:?port}
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../crates/amphora/tests/tpcb"
python3 ../../../../scripts/postgres-tpcb-answers.py "$HOST" "$PORT" statements.txt > answers.txt
python3 ../../../../scripts/postgres-tpcb-answers.py "$HOST" "$PORT" listener.txt > listener-answers.txt
