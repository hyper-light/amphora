#!/usr/bin/env python3
"""Lane Q6: the raw call chains behind q6_spins.py's keys: for the spin samples of each .folded
file, the TOP most frequent chains of the DEPTH frames above the spin, each frame's name with its
hash dropped and cut to WIDTH characters, so a key's lock and caller can be read exactly.
usage: q6_raw.py TOP DEPTH WIDTH FILE.folded..."""
import collections, os, re, sys

top, depth, width = int(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3])
SPIN = ("_mm_pause", "spin_loop", "__yield", "__isb")
for path in sys.argv[4:]:
    chains = collections.Counter()
    n_all = 0
    for line in open(path):
        stack, _, n = line.rstrip("\n").rpartition(" ")
        if not n.isdigit():
            continue
        n_all += int(n)
        frames = stack.split(";")
        if not any(s in frames[-1] for s in SPIN):
            continue
        chain = tuple(re.sub(r"::h[0-9a-f]{16}$", "", f)[:width] for f in frames[-depth:])
        chains[chain] += int(n)
    print(f"\n== {os.path.basename(path)}: samples {n_all} ==")
    for chain, c in chains.most_common(top):
        print(f"-- {100 * c / max(n_all, 1):.1f}% ({c})")
        for f in reversed(chain):
            print("   " + f)
