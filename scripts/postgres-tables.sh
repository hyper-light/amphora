#!/usr/bin/env bash
# The scanner's tables of docs/design/postgres.md section 3 (section 11's unit 4): flex 2.6.4 on the
# vendored REL_19_0 scan.l with PostgreSQL's flags (src/backend/parser/Makefile: -CF -p -p, and -b
# to prove there is no backing up), lifted into crates/pg-parser/src/scan_tables.rs.
# Linux with docker; nothing installed on the host. usage: scripts/postgres-tables.sh <work dir under ~/.cache>
set -euo pipefail
W=${1:?usage: postgres-tables.sh <work dir>}
ROOT=$(cd "$(dirname "$0")/.." && pwd)
mkdir -p "$W"
cp "$ROOT/vendor/postgres/src/backend/parser/scan.l" "$W/scan.l"
docker run --rm -e HU="$(id -u)" -e HG="$(id -g)" -v "$W":/t debian:trixie bash -ec '
  trap "chown -R $HU:$HG /t" EXIT
  apt-get -qq update >/dev/null && apt-get -qq install -y flex >/dev/null 2>&1
  cd /t && flex -CF -p -p -b -o scan.c scan.l 2>/dev/null
  flex --version | cut -d" " -f2 > flex.version
  [ "$(cat lex.backup)" = "No backing up." ] || { echo "scan.l backs up"; exit 1; }'
python3 -I "$ROOT/scripts/postgres-tables.py" "$W/scan.c" "$(cat "$W/flex.version")" "$(sha256sum "$W/scan.l" | cut -d" " -f1)" > "$ROOT/crates/pg-parser/src/scan_tables.rs"
echo "wrote crates/pg-parser/src/scan_tables.rs"
