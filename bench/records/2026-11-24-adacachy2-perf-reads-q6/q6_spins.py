#!/usr/bin/env python3
"""Lane Q6: every spin sample in AMPHORA_PROFILE .folded files, attributed to the wait it spins in.
A spin sample is one whose leaf is _mm_pause or spin_loop (Q4's definition, plus arm64's leaf).
Each is keyed by its thread, the nearest frame above the spin that is not the spin itself (the
lock or wait primitive), and the first four Amphora frames above that (its caller chain). Prints,
per file, each key's share of all samples; the keys sum to the spin total, which is printed too.
usage: q6_spins.py [--top N] FILE.folded..."""
import collections, os, re, sys

args = sys.argv[1:]
top = 30
if args[:1] == ["--top"]:
    top, args = int(args[1]), args[2:]

SPIN_LEAF = ("_mm_pause", "spin_loop", "__yield", "__isb")
SPIN_SELF = ("core::hint::", "core::core_arch::", "core::arch::", "cpu_relax", "SpinWait::spin", "Backoff::spin", "Backoff::snooze", "spin_loop")


def short(f):
    f = re.sub(r"::h[0-9a-f]{16}$", "", f)
    m = re.match(r"^<(.+?) as (.+?)>::(.+)$", f)
    if m:
        f = m.group(1) + "::" + m.group(3)
    for _ in range(4):
        f = re.sub(r"<[^<>]*>", "", f)
    parts = [p for p in f.split("::") if p]
    return "::".join(parts[-3:])


def ours(f):
    return "amphora" in f


for path in args:
    n_all = spin = 0
    keys = collections.Counter()
    threads = collections.Counter()
    spin_threads = collections.Counter()
    for line in open(path):
        stack, _, n = line.rstrip("\n").rpartition(" ")
        if not n.isdigit():
            continue
        n = int(n)
        frames = stack.split(";")
        thread = re.sub(r"\d+", "#", frames[0])
        n_all += n
        threads[thread] += n
        if not any(s in frames[-1] for s in SPIN_LEAF):
            continue
        spin += n
        spin_threads[thread] += n
        i = len(frames) - 1
        while i > 0 and any(s in frames[i] for s in SPIN_SELF + SPIN_LEAF):
            i -= 1
        prim = short(frames[i])
        chain = [short(f) for f in reversed(frames[1:i]) if ours(f)][:4]
        keys[(thread, prim, " < ".join(chain))] += n
    pct = lambda x: 100 * x / max(n_all, 1)
    print(f"\n== {os.path.basename(path)}: samples {n_all}, spin {spin} ({pct(spin):.1f}% of all) ==")
    print("threads (% of all samples, % of all that is that thread spinning): " + ", ".join(f"{t} {pct(c):.1f}/{pct(spin_threads[t]):.1f}" for t, c in threads.most_common(8)))
    shown = 0
    for (thread, prim, chain), c in keys.most_common(top):
        shown += c
        print(f"{pct(c):5.1f}% {c:6d} | {thread} | {prim} | {chain}")
    print(f"{pct(spin - shown):5.1f}% {spin - shown:6d} | the other {max(len(keys) - top, 0)} keys | sum {pct(spin):.1f}%")
