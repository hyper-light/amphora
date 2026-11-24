#!/usr/bin/env python3
"""Lane Q7: each AMPHORA_PROFILE .folded file's samples by what they were doing, for what limits
K0's mixed phase now:
- spin: the leaf is _mm_pause or spin_loop (Q4's and Q6's definition);
- the record registry (packed::Records): waiting, a frame of its lock or guard on the stack (the
  SpinLock's lock, the RwLock's read or write, a guard's drop); held, one of its methods
  (q6_hold.py's set) with no such frame;
- spin by the lock it spins in and that lock's caller (q6_split.py's rows), top 8;
- the frames by self (the leaf) and by total (anywhere on the stack, once a sample), roots on
  more than 90% of samples left out.
The bench's profiler drops samples whose PC is in libc, libgcc, pthread or the vdso, so a parked or
yielding waiter (futex, sched_yield) is in no count here; q6_sched.py's counters show those.
usage: q7_limit.py [--top N] FILE.folded..."""
import collections, os, re, sys

args = sys.argv[1:]
top = 15
if args[:1] == ["--top"]:
    top, args = int(args[1]), args[2:]
SPIN = ("_mm_pause", "spin_loop")
REG_LOCK = re.compile(r"^(lock|read|write|try_lock|try_read|try_write)<.*packed::Records|Guard<.*packed::Records")
REG_CS = re.compile(r"(^|[:;])(slots_of|decoded|covering|drop_after|publish_records|cache_records|record_pages|insert_record)(<|$)")


def name(f):
    f = re.sub(r"::h[0-9a-f]{16}$", "", f)
    m = re.match(r"^([A-Za-z_0-9{}#]+)<(.*)>$", f)
    if not m:
        return re.sub(r"<.*", "", f)[:60]
    arg = re.sub(r"<.*", "", m.group(2)).split("::")[-1]
    return f"{m.group(1)}<{arg}>"


for path in args:
    n_all = spin = reg_wait = reg_held = 0
    selfc, total, threads, spins = (collections.Counter() for _ in range(4))
    for line in open(path):
        stack, _, n = line.rstrip("\n").rpartition(" ")
        if not n.isdigit():
            continue
        n = int(n)
        frames = stack.split(";")
        n_all += n
        threads[re.sub(r"\d+", "#", frames[0])] += n
        body = frames[1:]
        if not body:
            continue
        selfc[name(body[-1])] += n
        for f in {name(f) for f in body}:
            total[f] += n
        if any(REG_LOCK.search(f) for f in body):
            reg_wait += n
        elif any(REG_CS.search(f) for f in body):
            reg_held += n
        if any(s in body[-1] for s in SPIN):
            spin += n
            i = len(body) - 1
            while i > 0 and any(s in body[i] for s in SPIN):
                i -= 1
            spins[(name(body[i]), name(body[i - 1]) if i > 0 else "-")] += n
    pct = lambda x: 100 * x / max(n_all, 1)
    print(f"\n== {os.path.basename(path)}: samples {n_all}; spin {pct(spin):.2f}%; record registry {pct(reg_wait + reg_held):.2f}% (waiting {pct(reg_wait):.2f}%, held {pct(reg_held):.2f}%)")
    print("threads: " + ", ".join(f"{t} {pct(c):.1f}%" for t, c in threads.most_common(6)))
    print("spin by lock | caller: " + "; ".join(f"{l} | {c} {pct(k):.2f}%" for (l, c), k in spins.most_common(8)))
    print("self: " + "; ".join(f"{f} {pct(c):.2f}%" for f, c in selfc.most_common(top)))
    print("total: " + "; ".join(f"{f} {pct(c):.1f}%" for f, c in [x for x in total.most_common() if x[1] <= 0.9 * n_all][: top + 10]))
