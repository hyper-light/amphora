#!/usr/bin/env python3
"""Lane Q3: from an AMPHORA_PROFILE .folded file, the share of samples whose stack holds LEAF (a
substring of the leaf frame), grouped by the chain of the DEPTH named frames above it.
usage: q3_callers.py FILE.folded LEAF [DEPTH]"""
import collections, re, sys

path, leaf, depth = sys.argv[1], sys.argv[2], int(sys.argv[3]) if len(sys.argv) > 3 else 4
skip = re.compile(r"^(core::|alloc::|std::|<core::|<alloc::|<std::|atomic_|_mm_pause|spin|try_lock|lock_|unlock|deref|drop_in_place|fetch_|load|store|compare_exchange|{closure)")
total = 0
hits = collections.Counter()
for line in open(path):
    stack, _, n = line.rstrip("\n").rpartition(" ")
    n = int(n)
    total += n
    frames = stack.split(";")
    if leaf not in frames[-1]:
        continue
    named = [re.sub(r"<.*", "", f) for f in reversed(frames[:-1]) if not skip.match(f)]
    hits[" < ".join(named[:depth])] += n
all_hits = sum(hits.values())
print(f"{path}: {all_hits} of {total} samples ({100 * all_hits / max(total, 1):.1f}%) end in {leaf}")
for chain, n in hits.most_common(12):
    print(f"  {100 * n / max(total, 1):5.1f}%  {chain}")
