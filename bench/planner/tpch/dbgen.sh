#!/usr/bin/env bash
# TPC-H's data for the planner bench (`amphora-bench --planner`, bench/planner/README.md): dbgen
# from electrum/tpch-dbgen at a fixed revision, built in a directory of its own under ~/.cache, its
# tables written at scale factor $1 to $2 with each line's closing '|' taken off, so every loader
# reads the same pipe-separated rows.
#
# usage: bench/planner/tpch/dbgen.sh <scale factor> <out dir>
set -euo pipefail
SF=${1:?scale factor}
OUT=${2:?out dir}
REV=32f1c1b92d1664dba542e927d23d86ffa57aa253
SRC=${TPCH_DBGEN_SRC:-$HOME/.cache/planner-bench/tpch-dbgen}
if [[ ! -d "$SRC/.git" ]]; then
  git clone -q https://github.com/electrum/tpch-dbgen "${SRC:?}"
fi
git -C "$SRC" checkout -q "$REV"
if [[ ! -x "$SRC/dbgen" ]]; then
  # Its K&R C predates C23, today's GCC default.
  make -C "$SRC" -s CC="gcc -std=gnu89 -w" >/dev/null
fi
test "$(git -C "$SRC" rev-parse HEAD)" = "$REV"
mkdir -p "$OUT"
cd "$SRC"
# dbgen reads an unset stat of a file it is to make, and then may open it without a mode: its
# files are made new, and given their mode after.
rm -f -- "${SRC:?}"/*.tbl
nice -n 10 ./dbgen -q -f -s "$SF"
chmod 0644 ./*.tbl
for t in region nation part supplier partsupp customer orders lineitem; do
  sed 's/|$//' "$t.tbl" >"$OUT/$t.tbl"
  rm -f -- "${SRC:?}/$t.tbl"
done
echo "$REV" >"$OUT/REVISION"
