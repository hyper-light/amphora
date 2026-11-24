#!/usr/bin/env bash
# Regenerates crates/pg-analysis/tests/{operators,casts,functions,statements}.txt: what PostgreSQL 19 answers
# (scripts/postgres-answers.sql), from a running 19 server in a container with the psql of its image.
# statements.txt's first three fields are the questions; only its answers change.
#
# usage: scripts/postgres-answers.sh <container>
set -euo pipefail
C=${1:?container}
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../crates/pg-analysis/tests"
cut -d'|' -f1-3 statements.txt | docker exec -i "$C" sh -c 'cat > /tmp/statements.in'
docker exec -i "$C" psql -h 127.0.0.1 -U postgres -AtqX -f - < ../../../scripts/postgres-answers.sql
for f in operators casts functions; do docker exec "$C" cat "/tmp/$f.txt" > "$f.txt"; done
docker exec "$C" cat /tmp/statements.out > statements.txt
docker exec "$C" psql -h 127.0.0.1 -U postgres -AtqXc 'select version()'
