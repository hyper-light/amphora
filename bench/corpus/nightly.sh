#!/usr/bin/env bash
# The whole corpus, nightly (G2 step 7, T-G2.7.3; .kiln/corpus.yaml): every source fetched at the
# revision sources.json pins, the corpus built and run on SQLite (corpus.sh), then run whole on
# every engine adapter amphora-corpus has (today Amphora alone), each record's time kept, and the
# gate's pool re-ranked from those times (subset.py). The proposed pool is an artifact, not a
# commit: bench/corpus/subset changes by a reviewed commit of it.
#
# usage: bench/corpus/nightly.sh <results dir>
#   CORPUS_SRC   where the sources are fetched (default ~/.cache/research-src; kept between nights)
#   CORPUS_WORK  where the corpus is built (default ~/.cache/amphora-corpus-nightly; emptied first)
#   CORPUS_PY    a Python with sqlglot and psycopg (default ~/.cache/corpus-venv/bin/python; made
#                when missing, sqlglot at the version sources.json names)
# Writes to <results dir>: record/ (corpus.sh's), times/<machine>/<date>.jsonl (amphora-corpus's),
# amphora.txt (its disagreements and totals), subset/ (the proposed pool) and order.diff (it against
# bench/corpus/subset/order.tsv). Exits non-zero when a stage fails; Amphora's disagreements are
# results, recorded, not a failure (the gate's pool holds the files it answers right).
set -euo pipefail
HERE=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(cd -- "$HERE/../.." && pwd)
RESULTS=$(realpath -m -- "${1:?usage: nightly.sh <results dir>}")
SRC=${CORPUS_SRC:-$HOME/.cache/research-src}
WORK=${CORPUS_WORK:-$HOME/.cache/amphora-corpus-nightly}
PY=${CORPUS_PY:-$HOME/.cache/corpus-venv/bin/python}
MACHINE=${RUNNER_NAME:-$(uname -n)}
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

stage "amphora-corpus (the gate's build: the times re-rank the gate's pool)"
(cd "$ROOT" && cargo build --locked -p amphora-corpus)
bin="$(cd "$ROOT" && cargo metadata --format-version 1 --no-deps | python3 -I -c 'import json, sys; print(json.load(sys.stdin)["target_directory"])')/debug/amphora-corpus"

# Every engine adapter amphora-corpus has, the whole corpus each (G2 step 4's harness).
for engine in amphora; do
  stage "the whole corpus on $engine"
  mkdir -p "$RESULTS/times/$MACHINE"
  set +e
  "$bin" run "$WORK/corpus" --times "$RESULTS/times/$MACHINE/$STAMP.jsonl" > "$RESULTS/$engine.txt"
  rc=$?
  set -e
  tail -1 "$RESULTS/$engine.txt"
  # 0: every record agrees; 1: some disagree (recorded); anything else: the run itself failed
  [[ $rc -le 1 ]] || { echo "$engine: the runner failed ($rc)" >&2; exit "$rc"; }
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
exit "$fit"
