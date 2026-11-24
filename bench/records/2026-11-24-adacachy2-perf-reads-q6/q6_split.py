#!/usr/bin/env python3
"""Lane Q6: every spin sample of each .folded file by the lock it spins in (the frame above the
spin, with its type argument's last path segment) and its caller (the next frame up), so the
rows sum to the spin total; then the Records lock's held-and-running share (q6_hold.py's CS set).
usage: q6_split.py FILE.folded..."""
import collections, os, re, sys
SPIN = ("_mm_pause", "spin_loop")
CS = re.compile(r"(^|[:;])(slots_of|decoded|covering|drop_after|publish_records|cache_records)(<|$)")
def name(f):
    m = re.match(r"^([A-Za-z_0-9{}#]+)<(.*)>$", f)
    if not m:
        return re.sub(r"<.*", "", f)[:50]
    arg = re.sub(r"<.*", "", m.group(2)).split("::")[-1]
    return f"{m.group(1)}<{arg}>"
for path in sys.argv[1:]:
    n_all = spin = held = 0
    rows = collections.Counter()
    for line in open(path):
        stack, _, n = line.rstrip("\n").rpartition(" ")
        if not n.isdigit():
            continue
        n = int(n); frames = stack.split(";"); n_all += n
        if any(s in frames[-1] for s in SPIN):
            spin += n
            i = len(frames) - 1
            while i > 0 and any(s in frames[i] for s in SPIN):
                i -= 1
            caller = name(frames[i - 1]) if i > 0 else "-"
            rows[(re.sub(r"\d+", "#", frames[0]), name(frames[i]), caller)] += n
        elif any(CS.search(f) for f in frames):
            held += n
    pct = lambda x: 100 * x / max(n_all, 1)
    print(f"\n{os.path.basename(path)}: samples {n_all}, spin {spin} = {pct(spin):.2f}%, Records held-and-running {pct(held):.2f}%")
    for (t, lock, caller), c in rows.most_common():
        if pct(c) >= 0.05:
            print(f"  {pct(c):6.2f}% {t} | {lock} | {caller}")
    small = sum(c for c in rows.values() if pct(c) < 0.05)
    print(f"  {pct(small):6.2f}% the {sum(1 for c in rows.values() if pct(c) < 0.05)} rows under 0.05%")
