#!/usr/bin/env python3
"""The corpus on both storages, compared (goal "Many clients write through SQL at once, safely":
the whole corpus gives the same answers on both storage engines; reads as fast as the old storage,
writes 3 times faster). bench/corpus/storages.sh runs `amphora-corpus run` on the pager's B-trees
and with AMPHORA_STORAGE=versioned, twice: in memory (the gate's database), and on a file on the
disk under test (`--disk`: WAL, `synchronous = FULL`, the commit bench's durability). This reads
the four runs and prints:

  differ memory|disk <file>:<line> ...  a line for each record whose answer isn't the same on both
  answers: SAME|DIFFER|PARTIAL ...  the answers' verdict over the in-memory runs: SAME only when
                             every record of the pool's files was reached by both runs and answered
                             the same; PARTIAL when none differs but a record is in one run only or
                             unreached (a file stopped short, a budget): not the whole corpus
  answers on disk: SAME|DIFFER|PARTIAL ...  the same over the two runs on the disk
  reads: MEETS|MISSES ...    query records on the disk: versioned's time over the pager's (the bar:
                             no slower)
  writes: MEETS|MISSES|UNSETTLED ...  INSERT, UPDATE, DELETE and REPLACE statements on the disk:
                             the pager's time over versioned's (the bar: 3 times faster)

The times are the disk runs': in memory the versioned storage runs on its memory disk, so its
times would compare the two storages' code, not a disk's (T-T1.4.1).

A record's answer is its outcome (agree, disagree, untranslatable, ...) and, for one that disagrees
with SQLite, the answer the run printed for it. Times are compared record by record, over the
records both runs answered as SQLite does: the geometric mean of the per-record ratios with its 95%
interval (the conventional level; the normal approximation to the mean of the log ratios, so a
record counts once whatever its size), and the ratio of the totals beside it. A bar is met when the
interval says so: reads miss only when the interval lies wholly above 1 (shown slower); writes meet
when it lies wholly at or above 3, miss when wholly below, and are unsettled between (run again).

Each pair (in memory, on the disk) must name one client and one platform, the same for both of its
runs, and the two pairs one platform (each --times line names them): runs from another client or
machine aren't a comparison but experiment error, refused with exit 2 before anything is compared.
Each pair's client and platform head the verdicts. Exits 0 when both pairs' answers are the same
over every record, 1 when either pair's differ or are partial.

usage: storages.py <pool dir> <memory pager run> <memory versioned run> <disk pager run> <disk versioned run>
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
    """((file, line) -> (outcome, ns), {(client, platform)}), from --times' record lines (a file's
    own line has `file_ns`) and the client and platform every line names."""
    got, where = {}, set()
    with open(os.path.join(run, "times.jsonl"), encoding="utf-8") as f:
        for line in f:
            r = json.loads(line)
            where.add((r.get("client"), r.get("platform")))
            if "outcome" in r:
                got[(r["file"], r["line"])] = (r["outcome"], r["ns"])
    return got, where


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


def pair(label, pager_dir, versioned_dir):
    """One pair's runs (label "memory" or "disk"): (pager, versioned, (client, platform)), or None,
    said why, when its two runs don't name one client and platform, the same."""
    (pager, pager_where), (versioned, versioned_where) = outcomes(pager_dir), outcomes(versioned_dir)
    if len(pager_where) != 1 or pager_where != versioned_where:
        print(f"refused: the {label} runs' clients and platforms differ (pager {sorted(pager_where, key=str)},"
              f" versioned {sorted(versioned_where, key=str)}): not a comparison")
        return None
    (where,) = pager_where
    return pager, versioned, where


def answers(label, pager, versioned, pager_dir, versioned_dir):
    """Prints each record whose answer differs between a pair's runs: (differ, reached by both, in
    one run only, unreached by either)."""
    pager_shown, versioned_shown = shown(pager_dir), shown(versioned_dir)
    both = sorted(set(pager) & set(versioned))
    differ = 0
    for key in both:
        (po, _), (vo, _) = pager[key], versioned[key]
        ps, vs = pager_shown.get(key, ""), versioned_shown.get(key, "")
        if po != vo or ps != vs:
            differ += 1
            print(f"differ {label} {key[0]}:{key[1]}: pager {po}, versioned {vo}")
            for side, text in (("pager", ps), ("versioned", vs)):
                if text:
                    print(f"  {side}:\n{text}")
    reached = sum(1 for k in both if pager[k][0] != "unreached" and versioned[k][0] != "unreached")
    return differ, reached, len(set(pager) ^ set(versioned)), len(both) - reached


def main():
    if len(sys.argv) != 6:
        sys.exit(__doc__)
    pool, mem_pager, mem_versioned, disk_pager, disk_versioned = sys.argv[1:]
    memory, disk = pair("memory", mem_pager, mem_versioned), pair("disk", disk_pager, disk_versioned)
    if memory is None or disk is None:
        return 2
    if memory[2][1] != disk[2][1]:
        print(f"refused: the memory runs' platform {memory[2][1]} isn't the disk runs' {disk[2][1]}: not one machine")
        return 2
    for label, (_, _, (client, platform)) in (("memory", memory), ("disk", disk)):
        print(f"{label}: client {client}, platform {platform}, both runs")
    mem = answers("memory", memory[0], memory[1], mem_pager, mem_versioned)
    on_disk = answers("disk", disk[0], disk[1], disk_pager, disk_versioned)
    # The whole corpus answers the same only when every record was reached by both runs (T-T1.4.1):
    # a record in one run only, or unreached, is one whose answers weren't compared.
    whole = []
    for head, (differ, reached, one_side, unreached) in (("answers", mem), ("answers on disk", on_disk)):
        verdict = "DIFFER" if differ else "PARTIAL" if one_side or unreached else "SAME"
        whole.append(verdict == "SAME")
        print(f"{head}: {verdict}: {differ} of {reached} records reached by both differ"
              f" ({one_side} records in one run only, {unreached} unreached)")
    # The times are the disk's.
    pager, versioned = disk[0], disk[1]
    both = sorted(set(pager) & set(versioned))
    kind = kinds(pool, sorted({f for f, _ in both}))
    agree = [k for k in both if pager[k][0] == "agree" and versioned[k][0] == "agree"]
    reads = ratio([(versioned[k][1], pager[k][1]) for k in agree if kind.get(k) == "read"])
    writes = ratio([(pager[k][1], versioned[k][1]) for k in agree if kind.get(k) == "write"])
    if reads is None:
        print("reads: UNSETTLED: fewer than two query records answered right on both, on the disk")
    else:
        g, lo, hi, total, n = reads
        print(f"reads: {'MISSES' if lo > 1 else 'MEETS'}: on the disk, versioned takes {g:.3f}x the pager's time"
              f" (95% {lo:.3f} to {hi:.3f}; totals {total:.3f}x; {n} queries; bar: no more than 1x)")
    if writes is None:
        print("writes: UNSETTLED: fewer than two write statements answered right on both, on the disk")
    else:
        g, lo, hi, total, n = writes
        verdict = "MEETS" if lo >= 3 else "MISSES" if hi < 3 else "UNSETTLED"
        print(f"writes: {verdict}: on the disk, versioned is {g:.3f}x as fast as the pager"
              f" (95% {lo:.3f} to {hi:.3f}; totals {total:.3f}x; {n} statements; bar: 3x)")
    return 0 if all(whole) else 1


if __name__ == "__main__":
    sys.exit(main())
