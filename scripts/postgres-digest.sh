#!/usr/bin/env bash
# The oracle's answers checked in (crates/pg-oracle/corpus.digest; crates/pg-oracle/src/digest.rs):
# builds PostgreSQL's oracle from vendor/postgres's commit in docker (scripts/postgres-oracle-extract.sh)
# and writes every regression statement's tree and tokens answers into the digest, so `cargo test`
# compares the Rust parser to 19 with no C. With --check it fails instead when the checked-in digest
# isn't the oracle's answer; kiln CI runs that whenever vendor/postgres changes (.kiln/postgres-oracle.yaml).
# PG_ORACLE=<built pg-oracle> skips the build. Linux with docker.
# usage: scripts/postgres-digest.sh <work dir under ~/.cache> [--check]
set -euo pipefail
W=${1:?usage: postgres-digest.sh <work dir> [--check]}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
ORACLE=${PG_ORACLE:-$W/pg-oracle}
[ -n "${PG_ORACLE:-}" ] || bash "$ROOT/scripts/postgres-oracle-extract.sh" "$W"
DIGEST=$ROOT/crates/pg-oracle/corpus.digest
NEW=$W/corpus.digest
(cd "$ROOT" && cargo run --quiet --locked -p amphora-pg-oracle --bin pg-oracle-digest -- "$ORACLE") > "$NEW"
if [ "${2:-}" = --check ]; then
  if ! cmp -s "$NEW" "$DIGEST"; then
    echo "crates/pg-oracle/corpus.digest is not the oracle's answer: regenerate it with scripts/postgres-digest.sh"
    diff "$DIGEST" "$NEW" | head -40
    exit 1
  fi
  echo "corpus.digest is the oracle's answer for $(grep -vc '^@' "$DIGEST") statements"
else
  mv "$NEW" "$DIGEST"
  echo "wrote crates/pg-oracle/corpus.digest"
fi
