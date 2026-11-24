#!/usr/bin/env python3
"""Lane Q4: from AMPHORA_PROFILE .folded files named <way>-<round>-amphora-<statement>-<clients>,
the share of all CPU samples per way, statement and client count:
  spin        samples whose leaf is _mm_pause (any lock spun on)
  lookup      samples with a find_frame frame (the frame lookup, inclusive, whichever way)
  lookup spin samples whose leaf is _mm_pause under a find_frame frame (its lock)
  header      samples with a wal_header frame (the log's header read)
  log         samples with SharedLog::begin_read or SharedLog::end_read (Q3's compare-and-swap)
usage: q4_shares.py DIR"""
import collections, glob, os, re, sys

cells = collections.defaultdict(dict)
for path in sorted(glob.glob(os.path.join(sys.argv[1], "*.folded"))):
    m = re.match(r"(?P<way>[a-z]+)-(?P<round>\d+)-amphora-(?P<op>.+)-(?P<clients>\d+)\.folded$", os.path.basename(path))
    if not m:
        continue
    n_all = spin = lookup = lookup_spin = header = log = 0
    for line in open(path):
        stack, _, n = line.rstrip("\n").rpartition(" ")
        if not n.isdigit():
            continue
        n = int(n)
        frames = stack.split(";")
        n_all += n
        in_lookup = any("find_frame" in f for f in frames)
        if "_mm_pause" in frames[-1]:
            spin += n
            lookup_spin += n if in_lookup else 0
        lookup += n if in_lookup else 0
        header += n if any("wal_header" in f for f in frames) else 0
        log += n if any("SharedLog::begin_read" in f or "SharedLog::end_read" in f for f in frames) else 0
    pct = lambda x: 100 * x / max(n_all, 1)
    cells[(m["op"], int(m["clients"]))][(m["way"], int(m["round"]))] = (n_all, pct(spin), pct(lookup), pct(lookup_spin), pct(header), pct(log))

ways = ["locked", "filtered", "published", "optimistic"]
names = ["samples", "spin", "lookup", "lookup spin", "header", "log"]
for i, name in enumerate(names):
    print(f"\n{name} ({'samples' if i == 0 else '% of all samples'}), round 1 / round 2")
    print("| statement | clients | " + " | ".join(ways) + " |")
    print("|---" * (len(ways) + 2) + "|")
    for (op, clients), by in sorted(cells.items()):
        row = []
        for way in ways:
            got = [by.get((way, r)) for r in (1, 2)]
            row.append(" / ".join("-" if g is None else (f"{g[i]:.0f}" if i == 0 else f"{g[i]:.1f}") for g in got))
        print(f"| {op} | {clients} | " + " | ".join(row) + " |")
