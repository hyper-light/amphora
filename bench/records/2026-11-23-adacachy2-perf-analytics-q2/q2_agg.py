#!/usr/bin/env python3
"""Lane Q2: over every reader's census (q2_census.sh's AMPHORA_CACHE_CENSUS dir), the counts that ran
on a full cache: the first reads its pages; how many later ones still read the file, and how far
the hand stood from the head at the first. usage: q2_agg.py DIR..."""
import collections, os, re, sys

for d in sys.argv[1:]:
    files = collections.defaultdict(dict)
    for name in os.listdir(d):
        m = re.match(r"([0-9a-f]+)-([ab])(\d+)\.txt$", name)
        if m:
            files[m.group(1)][(m.group(2), int(m.group(3)))] = os.path.join(d, name)

    def head(p):
        lines = open(p).read().splitlines()
        h = dict(zip(lines[0].split()[::2], map(int, lines[0].split()[1::2])))
        h.update(dict(zip(lines[1].split()[::2], map(int, lines[1].split()[1::2]))))
        return h

    readers, thrashing, later, hands = 0, 0, [], []
    for f in files.values():
        seq = []
        for n in range(4):
            if ("b", n) in f and ("a", n) in f:
                hb, ha = head(f[("b", n)]), head(f[("a", n)])
                seq.append((hb["len"], hb["capacity"], hb["hand"], ha["file_reads"] - hb["file_reads"]))
        full = [s for s in seq if s[0] == s[1]]
        if not full:
            continue
        readers += 1
        hands.append(full[0][0] - full[0][2])
        reads = [s[3] for s in full[1:]]
        later.extend(reads)
        thrashing += any(r > 0 for r in reads)
    hands.sort()
    print(os.path.basename(d), f"readers with a count on a full cache {readers} of {len(files)}; readers whose later counts read the file {thrashing};",
          f"later counts {len(later)}, reading {sum(1 for r in later if r > 0)}, pages read {sum(later)};",
          f"hand to head at the first: min {hands[0] if hands else '-'} median {hands[len(hands) // 2] if hands else '-'} max {hands[-1] if hands else '-'}")
