#!/usr/bin/env python3
"""The gate's pool (G2 step 7, T-G2.7.1): the corpus files the gate runs on Amphora, chosen from the
times machines measured running them (`amphora-corpus run <pool> --times <file>`), never by hand.

  required  files that, between them, reach every inventory entry with records Amphora answers as
            SQLite does: chosen greedily by entries not yet reached per measured second
  fill      then files by query types (tagger.type_key) not yet covered per measured second, until
            the fastest machine's measured times fill the budget

Only a file every run answered whole and right, its settles all reached, is chosen: the gate runs
a file whole and fails on any disagreement or failed settle, so a file Amphora gets wrong anywhere waits until it's fixed (subset.json lists
the entries no such file reaches yet). A file's measured second is the median of every complete run
of it on every machine (its `file_ns`: fresh database and fixture included); the budget is filled
by the fastest machine's own medians (the machine whose times over the files every machine ran sum
least), the median of the others where it hasn't run a file. The budget is the plan's requirement
value: "the subset runs in the gate in under 2 minutes" (G2 step 7), passed as --budget-s. The
runner holds slower machines to the budget as they run: fill files stop at its deadline.

Each times file is one run, named by the machine it ran on: <dir>/<machine>/<run>.jsonl (the gate's
and the nightly's artifacts keep them so); a directory given is searched for them.

usage: subset.py <features.json> <corpus dir> <out dir> --budget-s <seconds> --times <file|dir>...
                 [--jobs N]
  writes <out dir>: the chosen files (and the fixtures, <source>/base.test, they need), order.tsv
  (`required|fill<TAB>path`, the runner's order) and subset.json (each file's role, times and what
  it adds; the machines; the entries not reached). Exits 1 when the required files alone don't fit
  the budget on the fastest machine (the pool is still written: the gate shows it).
"""
import json
import multiprocessing
import os
import shutil
import statistics
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import slt  # noqa: E402
import tagger as tagmod  # noqa: E402

TAGGER = None


def args(argv):
    pos, opts, times = [], {}, []
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--times":
            i += 1
            while i < len(argv) and not argv[i].startswith("--"):
                times.append(argv[i])
                i += 1
            continue
        if a.startswith("--"):
            opts[a] = argv[i + 1]
            i += 2
            continue
        pos.append(a)
        i += 1
    if len(pos) != 3 or "--budget-s" not in opts or not times:
        sys.exit(__doc__)
    return pos, float(opts["--budget-s"]), times, int(opts.get("--jobs", "4"))


def times_files(paths):
    """(machine, path) for every run: a file's machine is its directory's name."""
    out = []
    for p in paths:
        if os.path.isdir(p):
            for d, _, files in os.walk(p):
                out += [(os.path.basename(d), os.path.join(d, f)) for f in sorted(files) if f.endswith(".jsonl")]
        else:
            out.append((os.path.basename(os.path.dirname(os.path.abspath(p))), p))
    return out


def read_runs(runs):
    """Per file: whether every run answered every record it reached right, the lines that ran, and
    each machine's complete runs' times (a run its deadline cut short measures part of the file)."""
    bad, cut, lines, file_ns, settled = set(), set(), {}, {}, set()
    for machine, path in runs:
        run_cut, run_ns = set(), {}
        with open(path) as f:
            rows = f.read().split("\n")
        for i, row in enumerate(rows):
            row = row.strip()
            if not row:
                continue
            try:
                r = json.loads(row)
            except json.JSONDecodeError:
                if i >= len(rows) - 2:  # a run stopped mid-write (killed at a timeout) ends in part of a line
                    continue
                raise
            rel = r["file"]
            if "file_ns" in r:
                # a failed settle fails the gate's run as a disagreement does; a run that doesn't count
                # them (no `unsettled`) measures the time only
                if r.get("unsettled", 0):
                    bad.add(rel)
                elif "unsettled" in r:
                    settled.add(rel)
                run_ns[rel] = r["file_ns"]
            elif r["outcome"] == "agree":
                lines.setdefault(rel, set()).add(r["line"])
            elif r["outcome"] == "unreached":
                run_cut.add(rel)
            else:
                bad.add(rel)
        cut |= run_cut
        for rel, ns in run_ns.items():
            if rel not in run_cut:
                file_ns.setdefault(rel, {}).setdefault(machine, []).append(ns)
    return bad, cut, lines, file_ns, settled


def credit_of(corpus):
    """written/index.json: each written file's entry, credited from its cases' first line on
    (build_corpus.py's rule)."""
    path = os.path.join(corpus, "written", "index.json")
    return json.load(open(path)) if os.path.exists(path) else {}


def tag_file(job):
    """A file's entries and query types, from the records that ran right."""
    global TAGGER
    features, corpus, rel, ran, credit = job
    if TAGGER is None:
        TAGGER = tagmod.Tagger(features)
    entries, types = set(), set()
    for kind, rec in slt.parse(os.path.join(corpus, rel)):
        if kind != "record" or rec.line not in ran:
            continue
        feats, shape = TAGGER.tag(rec.sql)
        if credit and rec.line >= credit["from_line"]:
            feats.add(credit["entry"])
        entries |= feats
        types.add(tagmod.type_key(feats, shape))
    return rel, entries, types


def greedy(cands, gain_of, cost, budget_left=None):
    """Picks files by gain per measured second until none gains (and, with a budget, until none
    fits): each pick re-scores the rest, as covering one thing lowers what the others add."""
    left = set(cands)
    while left:
        best, best_score = None, 0.0
        for rel in sorted(left):
            if budget_left is not None and cost[rel] > budget_left:
                continue
            g = gain_of(rel)
            score = g / cost[rel] if cost[rel] > 0 else float(g)
            if g and score > best_score:
                best, best_score = rel, score
        if best is None:
            break
        left.discard(best)
        if budget_left is not None:
            budget_left -= cost[best]
        yield best


def main():
    (features, corpus, out), budget_s, paths, jobs = args(sys.argv[1:])
    runs = times_files(paths)
    if not runs:
        sys.exit(f"no times files (<machine>/<run>.jsonl) under {paths}")
    bad, cut, lines, file_ns, settled = read_runs(runs)
    machines = sorted({m for ms in file_ns.values() for m in ms})
    ok = sorted(rel for rel in file_ns if rel in settled and rel not in bad and os.path.exists(os.path.join(corpus, rel)))

    # The fastest machine: least total over the files every machine ran whole.
    common = [rel for rel in ok if all(m in file_ns[rel] for m in machines)]
    fastest = min(machines, key=lambda m: sum(statistics.median(file_ns[rel][m]) for rel in common))
    rank_s = {rel: statistics.median([ns for v in file_ns[rel].values() for ns in v]) / 1e9 for rel in ok}
    fill_s = {}
    for rel in ok:
        mine = file_ns[rel].get(fastest)
        fill_s[rel] = statistics.median(mine) / 1e9 if mine else rank_s[rel]

    entry_ids = [f["id"] for f in json.load(open(features))["features"]]
    credit = credit_of(corpus)
    jobs_in = [(features, corpus, rel, lines.get(rel, set()), credit.get(rel)) for rel in ok]
    with multiprocessing.get_context("spawn").Pool(jobs) as pool:
        tagged = {rel: (e, t) for rel, e, t in pool.imap_unordered(tag_file, jobs_in, chunksize=8)}
    wanted = set(entry_ids)

    reached, covered, files = set(), set(), []
    # required: every entry Amphora answers right somewhere, at the least measured cost
    for rel in greedy([r for r in ok if tagged[r][0] & wanted], lambda r: len((tagged[r][0] & wanted) - reached), rank_s):
        new_e = (tagged[rel][0] & wanted) - reached
        new_t = tagged[rel][1] - covered
        reached |= new_e
        covered |= tagged[rel][1]
        files.append({"file": rel, "role": "required", "entries_added": len(new_e), "types_added": len(new_t), "rank_s": rank_s[rel], "fastest_s": fill_s[rel]})
    spent = sum(f["fastest_s"] for f in files)
    fits = spent <= budget_s
    # fill: query types not yet covered per measured second, until the budget is spent
    taken = {f["file"] for f in files}
    for rel in greedy([r for r in ok if r not in taken], lambda r: len(tagged[r][1] - covered), rank_s, budget_left=budget_s - spent) if fits else ():
        new_t = tagged[rel][1] - covered
        covered |= tagged[rel][1]
        files.append({"file": rel, "role": "fill", "entries_added": 0, "types_added": len(new_t), "rank_s": rank_s[rel], "fastest_s": fill_s[rel]})
        spent += fill_s[rel]

    # the pool, written afresh: a previous pool (its order.tsv there) is replaced, nothing else is
    if os.path.exists(out):
        if not os.path.exists(os.path.join(out, "order.tsv")):
            sys.exit(f"{out} exists and isn't a pool (no order.tsv): not replacing it")
        shutil.rmtree(out)
    os.makedirs(out)
    fixtures = set()
    for f in files:
        rel = f["file"]
        dest = os.path.join(out, rel)
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        shutil.copyfile(os.path.join(corpus, rel), dest)
        base = os.path.join(rel.split("/")[0], "base.test")
        if base != rel and os.path.exists(os.path.join(corpus, base)):
            fixtures.add(base)
    for base in sorted(fixtures):
        shutil.copyfile(os.path.join(corpus, base), os.path.join(out, base))
    with open(os.path.join(out, "order.tsv"), "w") as o:
        for f in files:
            o.write(f"{f['role']}\t{f['file']}\n")
    unreached = sorted(wanted - reached)
    report = {
        "what": "The gate's pool (G2 step 7): bench/corpus/subset.py's choice from the runs below. order.tsv is the runner's order.",
        "budget_s": budget_s,
        "budget_from": "G2 step 7: the subset runs in the gate in under 2 minutes",
        "runs": [{"machine": m, "times": os.path.basename(p)} for m, p in runs],
        "machines": machines,
        "fastest": fastest,
        "candidates": {"measured": len(file_ns), "answered_right_whole": len(ok), "with_a_disagreement": len(bad), "cut_by_a_deadline": len(cut - set(ok))},
        "required_s_on_fastest": sum(f["fastest_s"] for f in files if f["role"] == "required"),
        "pool_s_on_fastest": spent,
        "entries": len(wanted),
        "entries_reached": len(reached),
        "entries_not_reached": unreached,
        "query_types": len(covered),
        "files": files,
    }
    with open(os.path.join(out, "subset.json"), "w") as o:
        json.dump(report, o, indent=1)
        o.write("\n")
    print(f"{len(files)} files ({sum(f['role'] == 'required' for f in files)} required): {len(reached)} of {len(wanted)} entries, "
          f"{len(covered)} query types, {spent:.1f} s of {budget_s:.0f} on {fastest}; {len(unreached)} entries no right-answered file reaches")
    if not fits:
        print(f"the required files alone take {spent:.1f} s on {fastest}, past the {budget_s:.0f} s budget")
        sys.exit(1)


if __name__ == "__main__":
    main()
