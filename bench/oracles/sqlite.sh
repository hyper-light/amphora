#!/usr/bin/env bash
# The oracles' reference SQLite (G2 step 3): release 3.54.0's amalgamation, compiled with the options
# the system's library of that release reports (`PRAGMA compile_options` through Arch's 3.54.0-1
# libsqlite3.so.0, which the correctness corpus is checked on; its sqlite3 shell reports more, its
# own) and one more, SQLITE_LIKE_DOESNT_MATCH_BLOBS: SQLite recommends it and Amphora adopts it, so a
# BLOB matches no LIKE or GLOB (docs/design/planner.md, decision 3). The oracles then hold Amphora to
# the SQLite whose semantics it claims, not to a choice it made on SQLite's own advice. Prints the
# library's path; pass it to `--oracle --sqlite`.
#
# usage: bench/oracles/sqlite.sh [dir]   (default ~/.cache/sqlite-oracle; reuses a library built there)
set -euo pipefail
# made first, then named whole: macOS has no `realpath -m`
mkdir -p -- "${1:-$HOME/.cache/sqlite-oracle}"
DIR=$(cd -- "${1:-$HOME/.cache/sqlite-oracle}" && pwd)
NAME=sqlite-amalgamation-3540000
URL=https://sqlite.org/2026/$NAME.zip
# As downloaded on 2026-10-24: its SQLITE_SOURCE_ID is check-in be8d059e9a49 of 2026-10-09, the one
# the system's 3.54.0 reports.
SHA3=7b670a62fdfbd672b75fef004cb703c8a3e87d3a5cc7d675b4a08337004a2d93
LIB=$DIR/libsqlite3-oracle-3540000.so
OPTIONS=(
  -DSQLITE_SECURE_DELETE -DSQLITE_DIRECT_OVERFLOW_READ -DHAVE_ISNAN -DSQLITE_ENABLE_COLUMN_METADATA
  -DSQLITE_ENABLE_DBSTAT_VTAB -DSQLITE_ENABLE_FTS3 -DSQLITE_ENABLE_FTS3_PARENTHESIS -DSQLITE_ENABLE_FTS4
  -DSQLITE_ENABLE_FTS5 -DSQLITE_ENABLE_MATH_FUNCTIONS -DSQLITE_ENABLE_PERCENTILE
  -DSQLITE_ENABLE_PREUPDATE_HOOK -DSQLITE_ENABLE_RTREE -DSQLITE_ENABLE_SESSION -DSQLITE_ENABLE_STAT4
  -DSQLITE_ENABLE_STMTVTAB -DSQLITE_ENABLE_UNLOCK_NOTIFY -DSQLITE_MAX_VARIABLE_NUMBER=250000
  -DSQLITE_MAX_EXPR_DEPTH=10000 -DSQLITE_THREADSAFE=1
  -DSQLITE_LIKE_DOESNT_MATCH_BLOBS
)
if [ ! -f "$LIB" ]; then
  mkdir -p "$DIR/dl" "$DIR/src"
  ZIP=$DIR/dl/$NAME.zip
  [ -f "$ZIP" ] || curl -sSf -o "$ZIP" "$URL"
  # Python's hashlib, which every platform's python3 has: macOS's openssl (LibreSSL) has no SHA3.
  got=$(python3 -I -c 'import hashlib, sys; print(hashlib.sha3_256(open(sys.argv[1], "rb").read()).hexdigest())' "$ZIP")
  [ "$got" = "$SHA3" ] || { echo "sqlite.sh: $ZIP has SHA3-256 $got, not $SHA3" >&2; exit 1; }
  python3 -I -c 'import sys, zipfile; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])' "$ZIP" "$DIR/src"
  nice -n 19 cc -O2 -fPIC -shared "${OPTIONS[@]}" -o "$LIB.tmp" "$DIR/src/$NAME/sqlite3.c" -lm -lpthread
  mv "$LIB.tmp" "$LIB"
fi
# The library's options are the system's, less the compiler's name, plus LIKE_DOESNT_MATCH_BLOBS,
# where the system's library is this release (elsewhere, a CI pod's, there is nothing to hold it to).
python3 -I - "$LIB" <<'PY'
import ctypes, sqlite3, sys
if sqlite3.sqlite_version != "3.54.0":
    sys.exit(0)
lib = ctypes.CDLL(sys.argv[1])
db, st = ctypes.c_void_p(), ctypes.c_void_p()
assert lib.sqlite3_open(b":memory:", ctypes.byref(db)) == 0
assert lib.sqlite3_prepare_v2(db, b"PRAGMA compile_options", -1, ctypes.byref(st), None) == 0
lib.sqlite3_column_text.restype = ctypes.c_char_p
ours = set()
while lib.sqlite3_step(st) == 100:
    ours.add(lib.sqlite3_column_text(st, 0).decode())
system = {r[0] for r in sqlite3.connect(":memory:").execute("PRAGMA compile_options")}
strip = lambda s: {o for o in s if not o.startswith("COMPILER=")}
extra, missing = strip(ours) - strip(system), strip(system) - strip(ours)
if extra != {"LIKE_DOESNT_MATCH_BLOBS"} or missing:
    sys.exit(f"sqlite.sh: the library's options differ from the system's: more {sorted(extra)}, fewer {sorted(missing)}")
PY
echo "$LIB"
