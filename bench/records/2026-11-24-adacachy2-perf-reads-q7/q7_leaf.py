#!/usr/bin/env python3
"""Lane Q7: in each .folded file, the share of samples with any frame naming packed::Records (the
registry's lock, its guards' drops, its Arc, generic code over it), beside q7_limit.py's narrower
count; then, for the leaves matching LEAF (a regex), the TOP caller chains of the DEPTH frames
above the leaf, leaf last.
usage: q7_leaf.py LEAF TOP DEPTH FILE.folded..."""
import collections, os, re, sys

leaf_re, top, depth = re.compile(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3])


def name(f):
    f = re.sub(r"::h[0-9a-f]{16}$", "", f)
    m = re.match(r"^([A-Za-z_0-9{}#]+)<(.*)>$", f)
    if not m:
        return re.sub(r"<.*", "", f)[:50]
    args = [re.sub(r"<.*", "", a).split("::")[-1] for a in m.group(2).split(", ")]
    return f"{m.group(1)}<{','.join(args)}>"[:60]


for path in sys.argv[4:]:
    n_all = reg_any = leaves = 0
    chains = collections.Counter()
    for line in open(path):
        stack, _, n = line.rstrip("\n").rpartition(" ")
        if not n.isdigit():
            continue
        n = int(n)
        frames = stack.split(";")[1:]
        n_all += n
        if any("packed::Records" in f for f in frames):
            reg_any += n
        if frames and leaf_re.search(frames[-1]):
            leaves += n
            chains[" < ".join(name(f) for f in reversed(frames[-depth:]))] += n
    pct = lambda x: 100 * x / max(n_all, 1)
    print(f"\n== {os.path.basename(path)}: samples {n_all}; any frame naming packed::Records {pct(reg_any):.2f}%; leaves matching {leaf_re.pattern}: {pct(leaves):.2f}%")
    for c, k in chains.most_common(top):
        print(f"  {pct(k):5.2f}% {c}")
