#!/usr/bin/env python3
"""Lane Q6: for each AMPHORA_PROFILE .folded file, the share of all samples that spin on the log's
record registry (`SpinLock<packed::Records>`), that spin on the frame cache (`page_frames`'
`SpinLock<HashMap>`), and that run inside the registry's critical sections (a Records method on
the stack: slots_of, decoded, covering, drop_after, insert, publish_records, cache_records, with
no spin leaf), plus the leaves of those critical-section samples. Held-and-running time beside
spin time says whether spinners wait on a holder that runs or on one taken off its CPU.
usage: q6_hold.py FILE.folded..."""
import collections, os, re, sys

SPIN = ("_mm_pause", "spin_loop")
CS = re.compile(r"(^|[:;])(slots_of|decoded|covering|drop_after|publish_records|cache_records|record_pages|insert_record)(<|$)")
for path in sys.argv[1:]:
    n_all = spin = spin_rec = spin_cache = hold = 0
    leaves = collections.Counter()
    for line in open(path):
        stack, _, n = line.rstrip("\n").rpartition(" ")
        if not n.isdigit():
            continue
        n = int(n)
        frames = stack.split(";")
        n_all += n
        if any(s in frames[-1] for s in SPIN):
            spin += n
            if any(f.startswith("lock<amphora_engine::storage::packed::Records") for f in frames):
                spin_rec += n
            elif any(f == "page_frames" for f in frames):
                spin_cache += n
            continue
        if any(CS.search(f) for f in frames):
            hold += n
            leaves[re.sub(r"<.*", "", frames[-1])[:60]] += n
    pct = lambda x: 100 * x / max(n_all, 1)
    print(f"{os.path.basename(path)}: samples {n_all} spin {pct(spin):.1f}% records-spin {pct(spin_rec):.1f}% frame-cache-spin {pct(spin_cache):.1f}% records-held {pct(hold):.2f}%")
    print("   held leaves: " + ", ".join(f"{k} {pct(c):.2f}" for k, c in leaves.most_common(6)))
