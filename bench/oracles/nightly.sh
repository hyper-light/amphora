#!/usr/bin/env bash
# The oracles' nightly run (G2 step 3): `amphora-bench --oracle` for a night's budget, every oracle,
# the date its seed, into a directory of its own, against the reference SQLite sqlite.sh builds; its
# findings listed after. Exits 1 when a run finds a wrong answer or a panic (an error, Amphora
# refusing what SQLite runs, and an estimate CERT doubts are listed, not failed on).
#
# usage: bench/oracles/nightly.sh <bench binary> <out dir>
#   ORACLE_SECONDS  the budget (default 1800)
#   ORACLE_SEED     the seed (default the date, YYYYMMDD)
#   ORACLE_SQLITE   the reference library (default: what bench/oracles/sqlite.sh builds)
set -uo pipefail
HERE=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
BENCH=$1
# made first, then named whole: macOS has no `realpath -m`
mkdir -p -- "$2" || exit 2
OUT=$(cd -- "$2" && pwd) || exit 2
SQLITE=${ORACLE_SQLITE:-$("$HERE/sqlite.sh")} || exit 2
"$BENCH" --oracle --seconds "${ORACLE_SECONDS:-1800}" --seed "${ORACLE_SEED:-$(date +%Y%m%d)}" --reduce-seconds 20 \
  --features "$HERE/../corpus/features.json" --sqlite "$SQLITE" --out "$OUT" || exit 2
python3 - "$OUT/summary.json" <<'PY'
import json, sys
s = json.load(open(sys.argv[1]))
checks = sum(o["checks"] for o in s["oracles"].values())
print(f"{s['states']} databases, {checks} checks in {s['seconds']:.0f} s; {len(s['findings'])} findings ({s['duplicates']} more of the same)")
for name, o in s["oracles"].items():
    print(f"  {name:6} {o['checks']:7} checks, {o['passed']:7} passed, {o['failed']:4} failed" + (f", another plan in {o['plans_changed']}" if o["plans_changed"] else ""))
bad = {k: f for k, f in s["findings"].items() if f["kind"] in ("Wrong", "Panic", "Estimate")}
for k, f in sorted(s["findings"].items(), key=lambda kv: kv[1]["at_seconds"]):
    print(f"  {f['at_seconds']:7.1f} s  {k}  {f['kind']}: {f['detail']}")
sys.exit(1 if any(f["kind"] in ("Wrong", "Panic") for f in bad.values()) else 0)
PY
