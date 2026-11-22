#!/usr/bin/env python3
"""The corpus on both storages, compared (goal "Many clients write through SQL at once, safely":
the whole corpus gives the same answers on both storage engines; reads as fast as the old storage,
writes 3 times faster). bench/corpus/storages.sh runs `amphora-corpus run` once on the pager's
B-trees and once with AMPHORA_STORAGE=versioned; this reads the two runs and prints:

  differ <file>:<line> ...   one line for each record whose answer isn't the same on both
  answers: SAME|DIFFER ...   the answers' verdict over every record both runs reached
  reads: MEETS|MISSES ...    query records: versioned's time over the pager's (the bar: no slower)
  writes: MEETS|MISSES|UNSETTLED ...  INSERT, UPDATE, DELETE and REPLACE statements: the pager's
                             time over versioned's (the bar: 3 times faster)

A record's answer is its outcome (agree, disagree, untranslatable, ...) and, for one that disagrees
with SQLite, the answer the run printed for it. Times are compared record by record, over the
records both runs answered as SQLite does: the geometric mean of the per-record ratios with its 95%
interval (the conventional level; the normal approximation to the mean of the log ratios, so a
record counts once whatever its size), and the ratio of the totals beside it. A bar is met when the
interval says so: reads miss only when the interval lies wholly above 1 (shown slower); writes meet
when it lies wholly at or above 3, miss when wholly below, and are unsettled between (run again).

usage: storages.py <pool dir> <pager run dir> <versioned run dir>
  each run dir holds out.txt (the run's stdout) and times.jsonl (its --times)
"""
import json
import math
import os
import re
import statistics
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import slt  # noqa: E402

WRITE = re.compile(r"\b(INSERT|UPDATE|DELETE|REPLACE)\b", re.I)
HEAD = re.compile(r"^(\S.*?):(\d+): ")


def outcomes(run):
    """(file, line) -> outcome, from --times' record lines (a file's own line has `file_ns`)."""
    got = {}
    with open(os.path.join(run, "times.jsonl"), encoding="utf-8") as f:
        for line in f:
            r = json.loads(line)
            if "outcome" in r:
                got[(r["file"], r["line"])] = (r["outcome"], r["ns"])
    return got


def shown(run):
    """(file, line) -> what the run printed for a record that disagrees (its block on stdout)."""
    blocks, key = {}, None
    with open(os.path.join(run, "out.txt"), encoding="utf-8", errors="replace") as f:
        for line in f:
            m = HEAD.match(line)
            if m and not line.startswith("    "):
                key = (m.group(1), int(m.group(2)))
                blocks[key] = []
            elif key is not None and line.startswith("    "):
                # The engine's own name is the storage's only difference in the text: none here.
                blocks[key].append(line.rstrip("\n"))
            else:
                key = None
    return {k: "\n".join(v) for k, v in blocks.items()}


def kinds(pool, files):
    """(file, line) -> "read", "write" or "other", from the records in the pool's files."""
    kind = {}
    for rel in files:
        path = os.path.join(pool, rel)
        if not os.path.exists(path):
            continue
        for what, rec in slt.parse(path):
            if what != "record":
                continue
            if rec.kind == "query":
                k = "read"
            else:
                sql = re.sub(r"--[^\n]*", "", rec.sql).lstrip()
                first = sql.split(None, 1)[0].upper() if sql else ""
                k = "write" if first in ("INSERT", "UPDATE", "DELETE", "REPLACE") or (first == "WITH" and WRITE.search(sql)) else "other"
            kind[(rel, rec.line)] = k
    return kind


def ratio(pairs):
    """(geometric mean, 95% low, 95% high, total ratio, n) of num/den over pairs (num, den)."""
    logs = [math.log(a / b) for a, b in pairs if a > 0 and b > 0]
    n = len(logs)
    if n < 2:
        return None
    mean, sd = statistics.fmean(logs), statistics.stdev(logs)
    half = statistics.NormalDist().inv_cdf(0.975) * sd / math.sqrt(n)
    total = sum(a for a, _ in pairs) / sum(b for _, b in pairs)
    return math.exp(mean), math.exp(mean - half), math.exp(mean + half), total, n


def main():
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    pool, pager_dir, versioned_dir = sys.argv[1:]
    pager, versioned = outcomes(pager_dir), outcomes(versioned_dir)
    pager_shown, versioned_shown = shown(pager_dir), shown(versioned_dir)
    both = sorted(set(pager) & set(versioned))
    one_side = len(set(pager) ^ set(versioned))
    differ = 0
    for key in both:
        (po, _), (vo, _) = pager[key], versioned[key]
        ps, vs = pager_shown.get(key, ""), versioned_shown.get(key, "")
        if po != vo or ps != vs:
            differ += 1
            print(f"differ {key[0]}:{key[1]}: pager {po}, versioned {vo}")
            for side, text in (("pager", ps), ("versioned", vs)):
                if text:
                    print(f"  {side}:\n{text}")
    reached = sum(1 for k in both if pager[k][0] != "unreached")
    print(f"answers: {'SAME' if differ == 0 else 'DIFFER'}: {differ} of {reached} records reached by both differ"
          f" ({one_side} records in one run only)")
    kind = kinds(pool, sorted({f for f, _ in both}))
    agree = [k for k in both if pager[k][0] == "agree" and versioned[k][0] == "agree"]
    reads = ratio([(versioned[k][1], pager[k][1]) for k in agree if kind.get(k) == "read"])
    writes = ratio([(pager[k][1], versioned[k][1]) for k in agree if kind.get(k) == "write"])
    if reads is None:
        print("reads: UNSETTLED: fewer than two query records answered right on both")
    else:
        g, lo, hi, total, n = reads
        print(f"reads: {'MISSES' if lo > 1 else 'MEETS'}: versioned takes {g:.3f}x the pager's time"
              f" (95% {lo:.3f} to {hi:.3f}; totals {total:.3f}x; {n} queries; bar: no more than 1x)")
    if writes is None:
        print("writes: UNSETTLED: fewer than two write statements answered right on both")
    else:
        g, lo, hi, total, n = writes
        verdict = "MEETS" if lo >= 3 else "MISSES" if hi < 3 else "UNSETTLED"
        print(f"writes: {verdict}: versioned is {g:.3f}x as fast as the pager"
              f" (95% {lo:.3f} to {hi:.3f}; totals {total:.3f}x; {n} statements; bar: 3x)")
    return 0 if differ == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
