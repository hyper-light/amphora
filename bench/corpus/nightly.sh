#!/usr/bin/env bash
# The whole corpus, nightly (G2 step 7, T-G2.7.3; .kiln/corpus.yaml): every source fetched at the
# revision sources.json pins, the corpus built and run on SQLite (corpus.sh), then run whole on
# every engine adapter amphora-corpus has (G2 step 4: amphora_corpus::engines::ENGINES), each
# record's time kept, and the gate's pool re-ranked from Amphora's times (subset.py). The proposed pool is an artifact, not a
# commit: bench/corpus/subset changes by a reviewed commit of it.
#
# usage: bench/corpus/nightly.sh <results dir>
#   CORPUS_SRC   where the sources are fetched (default ~/.cache/research-src; kept between nights)
#   CORPUS_WORK  where the corpus is built (default ~/.cache/amphora-corpus-nightly; emptied first)
#   CORPUS_PY    a Python with sqlglot and psycopg (default ~/.cache/corpus-venv/bin/python; made
#                when missing, sqlglot at the version sources.json names)
#   CORPUS_BUDGET_S  the night's budget in seconds (.kiln/corpus.yaml sets it to its job's timeout):
#                each engine's run gets an equal share of what's left of it when it starts, and no
#                record runs longer than that share (the engine stops it: past the budget)
#   CORPUS_ENGINES   the engines to run (default: every one amphora-corpus has); the server engines
#                run in the bench's containers (bench/servers/<engine>.sh), started and stopped here
# Writes to <results dir>: record/ (corpus.sh's), times/<machine>/<date>.jsonl (Amphora's, which
# re-rank the pool), times-<engine>/<machine>/<date>.jsonl (each other engine's), <engine>.txt (each
# engine's disagreements and totals), unavailable.txt (the engines this machine couldn't run), subset/ (the proposed pool) and order.diff (it against
# bench/corpus/subset/order.tsv). Exits non-zero when a stage fails or an engine couldn't run here
# (after running every other); disagreements are results, recorded, not a failure (the gate's pool
# holds the files Amphora answers right).
set -euo pipefail
HERE=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(cd -- "$HERE/../.." && pwd)
RESULTS=$(realpath -m -- "${1:?usage: nightly.sh <results dir>}")
SRC=${CORPUS_SRC:-$HOME/.cache/research-src}
WORK=${CORPUS_WORK:-$HOME/.cache/amphora-corpus-nightly}
PY=${CORPUS_PY:-$HOME/.cache/corpus-venv/bin/python}
MACHINE=${RUNNER_NAME:-$(uname -n)}
BUDGET=${CORPUS_BUDGET_S:?set CORPUS_BUDGET_S to the night'"'"'s budget in seconds (its job'"'"'s timeout)}
read -r -a ENGINES <<< "${CORPUS_ENGINES:-sqlite duckdb amphora turso postgres orioledb cedardb mysql clickhouse}"
STAMP=$(date -u +%Y%m%dT%H%M%SZ)
stage() { echo "== $(date -Is) $*"; }
field() { python3 -I -c 'import json, sys; d = json.load(open(sys.argv[1])); print(d["sources"][sys.argv[2]].get(sys.argv[3], "") if len(sys.argv) > 3 else d[sys.argv[2]]["version"])' "$HERE/sources.json" "$@"; }

stage "sources at their pinned revisions"
mkdir -p "$SRC"
for s in slt duckdb cockroach postgres kiln; do
  repo=$(field "$s" repository)
  rev=$(field "$s" revision)
  dir="$SRC/$(field "$s" checkout)"
  [[ $s == kiln ]] && dir="$SRC/corpus-kiln"
  if [[ "$(git -C "$dir" rev-parse HEAD 2>/dev/null)" != "$rev" ]]; then
    [[ -d "$dir/.git" ]] || git init -q "$dir"
    git -C "$dir" fetch -q --depth 1 "$repo" "$rev"
    git -C "$dir" checkout -q --detach "$rev"
  fi
  echo "$s $(git -C "$dir" rev-parse HEAD)"
done

if [[ ! -x "$PY" ]]; then
  stage "the corpus's Python (sqlglot $(field translator))"
  python3 -m venv "$(dirname -- "$(dirname -- "$PY")")"
  "$PY" -m pip install -q "sqlglot==$(field translator)" "psycopg[binary]"
fi

stage "the corpus, built and run on SQLite"
rm -rf -- "${WORK:?}"
mkdir -p "$WORK" "$RESULTS"
CORPUS_SRC="$SRC" CORPUS_PY="$PY" KILN_REPO="$SRC/corpus-kiln" bash "$HERE/corpus.sh" "$WORK/corpus" "$RESULTS/record"
# sqllogictest's files run as they are: the pool takes them from the checkout
mkdir -p "$WORK/corpus/slt"
ln -sfn "$SRC/$(field slt checkout)/test" "$WORK/corpus/slt/test"

stage "each record translated once per dialect (G2 step 4: the harness reads them)"
"$PY" "$HERE/dialects.py" "$WORK/corpus"

stage "amphora-corpus (the gate's build: the times re-rank the gate's pool)"
(cd "$ROOT" && cargo build --locked -p amphora-corpus)
bin="$(cd "$ROOT" && cargo metadata --format-version 1 --no-deps | python3 -I -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')/debug/amphora-corpus"

# Every engine adapter amphora-corpus has, the whole corpus each (G2 step 4's harness).
server=""
stop_server() {
  if [[ -n "$server" ]]; then
    bash "$ROOT/bench/servers/$server.sh" stop || true
    server=""
  fi
}
trap stop_server EXIT
: > "$RESULTS/unavailable.txt"
left=${#ENGINES[@]}
for engine in "${ENGINES[@]}"; do
  share=$(( (BUDGET - SECONDS) / left ))
  left=$(( left - 1 ))
  stage "the whole corpus on $engine (${share} s)"
  [[ $share -gt 0 ]] || { echo "the night's budget is spent before $engine" | tee -a "$RESULTS/unavailable.txt"; continue; }
  times="$RESULTS/times-$engine/$MACHINE"
  [[ $engine == amphora ]] && times="$RESULTS/times/$MACHINE"
  mkdir -p "$times"
  if [[ -f "$ROOT/bench/servers/$engine.sh" ]]; then
    server=$engine
    bash "$ROOT/bench/servers/$engine.sh" start || true
  fi
  set +e
  "$bin" run "$WORK/corpus" --engine "$engine" --budget-s "$share" --times "$times/$STAMP.jsonl" > "$RESULTS/$engine.txt"
  rc=$?
  set -e
  stop_server
  tail -1 "$RESULTS/$engine.txt"
  # 0: every record agrees; 1: some disagree (recorded); 3: the engine can't run here (recorded,
  # and the night fails at its end); anything else: the run itself failed
  case $rc in
    0 | 1) ;;
    3) tail -1 "$RESULTS/$engine.txt" >> "$RESULTS/unavailable.txt" ;;
    *) echo "$engine: the runner failed ($rc)" >&2; exit "$rc" ;;
  esac
done

stage "the gate's pool, re-ranked from tonight's times"
set +e
python3 -I "$HERE/subset.py" "$HERE/features.json" "$WORK/corpus" "$RESULTS/subset" --budget-s 120 --times "$RESULTS/times"
fit=$?
set -e
diff -u "$ROOT/bench/corpus/subset/order.tsv" "$RESULTS/subset/order.tsv" > "$RESULTS/order.diff" || true
echo "order.tsv: $(grep -c '^[+-][rf]' "$RESULTS/order.diff" || true) lines change"
rm -rf -- "${WORK:?}"
stage done
if [[ -s "$RESULTS/unavailable.txt" ]]; then
  echo "engines that couldn't run here:" >&2
  cat "$RESULTS/unavailable.txt" >&2
  exit 3
fi
exit "$fit"
