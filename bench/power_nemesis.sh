#!/usr/bin/env bash
# AC-T1.8.5's simulator half (CP-T1.8; G12 step 4): the power-cut nemesis over 10,000 seeds in both
# commit classes, sharded across machines. Each shard runs a seed range of one class (the ignored
# tests a_member_power_cut_loses_no_acknowledged_<class>_write_over_10000_seeds in
# crates/amphora/src/database/replicated.rs) and writes one record; `merge` checks the shards cover
# the seeds once each and sums them. Records of a run go under bench/records/power-nemesis/<run>/.
#
# usage:
#   bench/power_nemesis.sh run CLASS FROM COUNT DIR
#       seeds FROM..FROM+COUNT of CLASS (replicated or persisted) on this machine, in release, on
#       AMPHORA_POWER_WORKERS threads (default its cores); the record goes to
#       DIR/CLASS-FROM-COUNT.json with this machine's name and the commit added, and the test's
#       output to DIR/CLASS-FROM-COUNT.log. Exit: the test's (1 when any seed failed; the record
#       names each failing seed with its account either way).
#   bench/power_nemesis.sh tally HARNESS STORAGE WRITERS CUTS FAILED RESULTS
#       the device power-cut harnesses' verdict (bench/powercut/linux.sh and macos.sh, T-T1.4.1,
#       AC-T1.4.4): sums the acknowledged and lost of RESULTS (a `--powercut check` line a cut) and
#       prints one JSON line with HOSTNAME and the commit. Exit 1 when a cut failed, an
#       acknowledged write was lost, or nothing was acknowledged.
#   bench/power_nemesis.sh merge DIR CLASS TOTAL
#       every DIR/CLASS-*.json together: exit 1 unless they cover seeds 0..TOTAL exactly once, on
#       one commit, with no failed seed, some write acknowledged and some cut of a leader holding
#       a write unflushed; the replicated class with some write acknowledged while a majority's
#       raft logs held it unflushed, the persisted class with none. Writes DIR/CLASS.json, the
#       sums and every failing seed.
set -euo pipefail
HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd -- "$HERE/.." && pwd)"
PY=$(command -v python3 || command -v python)

case "${1:-}" in
  run)
    [[ $# -eq 5 ]] || { echo "usage: $0 run CLASS FROM COUNT DIR" >&2; exit 2; }
    class=$2 from=$3 count=$4 dir=$5
    case "$class" in replicated | persisted) ;; *) echo "CLASS is replicated or persisted" >&2; exit 2 ;; esac
    mkdir -p "$dir"
    dir="$(cd -- "$dir" && pwd)"
    out="$dir/$class-$from-$count"
    rm -f "$out.json"
    status=0
    AMPHORA_POWER_SEEDS="$from:$count" AMPHORA_POWER_RECORD="$out.json" \
      cargo test --release --locked --manifest-path "$ROOT/Cargo.toml" -p amphora --lib -- --ignored --exact --nocapture \
      "database::replicated::tests::a_member_power_cut_loses_no_acknowledged_${class}_write_over_10000_seeds" >"$out.log" 2>&1 || status=$?
    if [[ ! -s "$out.json" ]]; then
      echo "no record from the shard (exit $status); its log ends:" >&2
      tail -n 40 "$out.log" >&2
      exit 1
    fi
    "$PY" - "$out.json" "$(hostname)" "$(git -C "$ROOT" rev-parse HEAD)" <<'PY'
import json, sys
path, host, commit = sys.argv[1:]
with open(path, encoding="utf-8") as f:
    r = json.load(f)
r.update(host=host, commit=commit)
with open(path, "w", encoding="utf-8", newline="\n") as f:
    f.write(json.dumps(r) + "\n")
print(f"{r['class']} seeds {r['from']}..{r['from'] + r['count']}: {r['acknowledged']} acknowledged ({r['acknowledged_held']} with a "
      f"majority's raft logs held), {r['cuts']} cuts, {r['unflushed_leader']} of a leader holding a write unflushed in its "
      f"database and {r['unflushed_log_leader']} in its raft log, {len(r['failed'])} failed")
PY
    exit "$status"
    ;;
  tally)
    [[ $# -eq 7 ]] || { echo "usage: $0 tally HARNESS STORAGE WRITERS CUTS FAILED RESULTS" >&2; exit 2; }
    "$PY" - "$@" "$(hostname)" "$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)" <<'PY'
import json, sys
_, harness, storage, writers, cuts, failed, path, host, commit = sys.argv[1:]
acked = lost = 0
with open(path, encoding="utf-8") as f:
    for line in f:
        if line.strip():
            r = json.loads(line)
            acked += r["acked"]
            lost += r["lost"]
print(json.dumps({"harness": harness, "storage": storage, "writers": int(writers), "cuts": int(cuts),
                  "cuts_failed": int(failed), "acked": acked, "lost": lost, "host": host, "commit": commit}))
sys.exit(1 if int(failed) or lost or acked == 0 else 0)
PY
    ;;
  merge)
    [[ $# -eq 4 ]] || { echo "usage: $0 merge DIR CLASS TOTAL" >&2; exit 2; }
    "$PY" - "$2" "$3" "$4" <<'PY'
import glob, json, os, sys
d, cls, total = sys.argv[1], sys.argv[2], int(sys.argv[3])
shards = []
for p in sorted(glob.glob(os.path.join(d, f"{cls}-*-*.json"))):
    with open(p, encoding="utf-8") as f:
        shards.append(json.load(f))
problems = []
covered = [0] * total
for s in shards:
    for seed in range(s["from"], s["from"] + s["count"]):
        if 0 <= seed < total:
            covered[seed] += 1
        else:
            problems.append(f"seed {seed} lies outside 0..{total}")
gaps = [i for i, c in enumerate(covered) if c == 0]
twice = [i for i, c in enumerate(covered) if c > 1]
if gaps:
    problems.append(f"{len(gaps)} seeds no shard ran, the first {gaps[0]}")
if twice:
    problems.append(f"{len(twice)} seeds run by more than one shard, the first {twice[0]}")
commits = sorted({s.get("commit", "") for s in shards})
if len(commits) > 1:
    problems.append(f"shards on {len(commits)} commits: {commits}")
failed = sorted((f for s in shards for f in s["failed"]), key=lambda f: f["seed"])
sums = {k: sum(s.get(k, 0) for s in shards) for k in ("acknowledged", "acknowledged_held", "cuts", "unflushed_leader", "unflushed_log_leader")}
if failed:
    problems.append(f"{len(failed)} seeds failed, the first {failed[0]['seed']}")
if sums["acknowledged"] == 0 or sums["cuts"] == 0:
    problems.append("nothing was acknowledged or cut")
if sums["unflushed_leader"] == 0:
    problems.append("no cut hit a leader holding a write unflushed")
# The classes part at the raft log: the replicated class answers some write no majority's log held
# durable, the persisted class none (replication.md, "Fast acknowledgement").
if cls == "replicated" and sums["acknowledged_held"] == 0:
    problems.append("the replicated class acknowledged no write while a majority's raft logs held it unflushed")
if cls == "persisted" and sums["acknowledged_held"] != 0:
    problems.append(f"the persisted class acknowledged {sums['acknowledged_held']} writes while a majority's raft logs held them unflushed")
merged = {"class": cls, "seeds": total, "shards": len(shards), "commits": commits,
          "hosts": sorted({s.get("host", "") for s in shards}), **sums, "failed": failed, "problems": problems}
with open(os.path.join(d, f"{cls}.json"), "w", encoding="utf-8", newline="\n") as f:
    f.write(json.dumps(merged, indent=1) + "\n")
print(f"{cls}: {total} seeds in {len(shards)} shards, {sums['acknowledged']} acknowledged ({sums['acknowledged_held']} with a majority's "
      f"raft logs held), {sums['cuts']} cuts, {sums['unflushed_leader']} of a leader holding a write unflushed in its database and "
      f"{sums['unflushed_log_leader']} in its raft log, {len(failed)} failed")
for p in problems:
    print(f"  {p}", file=sys.stderr)
sys.exit(1 if problems else 0)
PY
    ;;
  *)
    echo "usage: $0 run CLASS FROM COUNT DIR | tally HARNESS STORAGE WRITERS CUTS FAILED RESULTS | merge DIR CLASS TOTAL" >&2
    exit 2
    ;;
esac
