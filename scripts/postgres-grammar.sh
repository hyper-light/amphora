#!/usr/bin/env bash
# The grammar's tables of docs/design/postgres.md section 3 (section 11's unit 5): bison 3.8.2 on the
# vendored REL_19_0 gram.y as PostgreSQL builds it (src/backend/parser/Makefile: `gram.c: BISONFLAGS += -d`),
# and again with -t for the symbol names, lifted into crates/pg-parser/src/gram_tables.rs; the lifter
# refuses if the two runs' parse tables differ.
# Linux with docker; nothing installed on the host. usage: scripts/postgres-grammar.sh <work dir under ~/.cache>
set -euo pipefail
W=${1:?usage: postgres-grammar.sh <work dir>}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
mkdir -p "$W"
cp "$ROOT/vendor/postgres/src/backend/parser/gram.y" "$W/gram.y"
docker run --rm -e HU="$(id -u)" -e HG="$(id -g)" -v "$W":/t debian:trixie bash -ec '
  trap "chown -R $HU:$HG /t" EXIT
  apt-get -qq update >/dev/null && apt-get -qq install -y bison >/dev/null 2>&1
  cd /t && bison --version | head -1 | sed "s/.* //" > bison.version
  [ "$(cat bison.version)" = 3.8.2 ] || { echo "bison $(cat bison.version), not 3.8.2"; exit 1; }
  bison -Wno-deprecated -d -o gram.c gram.y
  bison -Wno-deprecated -d -t -o gram-names.c gram.y'
python3 -I "$ROOT/scripts/postgres-grammar.py" "$W/gram.c" "$W/gram-names.c" "$(cat "$W/bison.version")" "$(sha256sum "$W/gram.y" | cut -d" " -f1)" > "$ROOT/crates/pg-parser/src/gram_tables.rs"
echo "wrote crates/pg-parser/src/gram_tables.rs"
