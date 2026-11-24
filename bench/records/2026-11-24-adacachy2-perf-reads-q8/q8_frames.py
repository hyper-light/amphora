#!/usr/bin/env python3
"""Lane Q8: every AMPHORA_PROFILE sample at the frame cache's lock (`WalSharedRuntime::frame_cache`,
a fast_lock::SpinLock<FxHashMap<u64, Vec<u64>>>) attributed to its callers, and every sample in a
function that takes that lock attributed to what it does there:
- waiting: the lock's `lock` frame on the stack (its swap, its load loop or spin_loop at the leaf),
  by the function that takes it and the callers above that (iterator plumbing skipped);
- in a taker, not waiting: by the frame under the taker (before the lock: the log's segment read
  guard; after it: the work done under the lock).
The bench's profiler drops samples whose PC is in libc, libgcc, pthread or the vdso, so a taker
parked on another lock while it holds this one (the records' write lock, a parking RwLock) is in
no count here; its spin before parking is.
usage: q8_frames.py FILE.folded..."""
import collections, os, re, sys

LOCK = re.compile(r"^lock<std::collections::hash::map::HashMap<u64, alloc::vec::Vec<u64")
TAKERS = {"cache_frame", "cache_record", "latest_frame", "page_frames", "rollback_cache", "scan_latest_frames"}
SPIN = ("_mm_pause", "spin_loop")
PLUMBING = re.compile(r"^(fold|for_each|from_iter|spec_extend|extend_trusted|collect|map_fold|call|call_once|call_mut|next|try_fold|\{closure#\d+\}|\{closure_env#\d+\})(<|$)")


def name(f):
    f = re.sub(r"::h[0-9a-f]{16}$", "", f)
    m = re.match(r"^([A-Za-z_0-9{}#]+)<(.*)>$", f)
    if not m:
        return re.sub(r"<.*", "", f)[:60]
    arg = re.sub(r"<.*", "", m.group(2)).split("::")[-1]
    return f"{m.group(1)}<{arg}>"


def callers(body, i, k):
    out, j = [], i - 1
    while j >= 0 and len(out) < k:
        n = name(body[j])
        if not PLUMBING.match(n) or not out:
            out.append(n)
        j -= 1
    return " < ".join(out)


for path in sys.argv[1:]:
    n_all = 0
    waiting, waiting_spin, by_caller, by_chain, held, threads = 0, 0, collections.Counter(), collections.Counter(), collections.Counter(), collections.Counter()
    for line in open(path):
        stack, _, n = line.rstrip("\n").rpartition(" ")
        if not n.isdigit():
            continue
        n = int(n)
        frames = stack.split(";")
        n_all += n
        body = frames[1:]
        names = [name(f) for f in body]
        i = next((k for k, f in enumerate(body) if LOCK.match(re.sub(r"::h[0-9a-f]{16}$", "", f))), None)
        if i is not None:
            waiting += n
            if body and any(s in body[-1] for s in SPIN):
                waiting_spin += n
            by_caller[names[i - 1] if i > 0 else "-"] += n
            by_chain[callers(body, i, 5)] += n
            threads[(re.sub(r"\d+", "#", frames[0]), names[i - 1] if i > 0 else "-")] += n
            continue
        j = max((k for k, f in enumerate(names) if f in TAKERS), default=None)
        if j is not None:
            child = names[j + 1] if j + 1 < len(names) else "(itself)"
            held[(names[j], child)] += n
    pct = lambda x: 100 * x / max(n_all, 1)
    print(f"\n== {os.path.basename(path)}: samples {n_all}; at the frame cache's lock {pct(waiting):.2f}% (leaf spin_loop or _mm_pause {pct(waiting_spin):.2f}%)")
    print("waiting, by the function taking the lock: " + "; ".join(f"{c} {pct(k):.2f}%" for c, k in by_caller.most_common()))
    print("waiting, by thread and taker: " + "; ".join(f"{t} | {c} {pct(k):.2f}%" for (t, c), k in threads.most_common(8)))
    print("waiting, by callers (taker < its callers):")
    for c, k in by_chain.most_common(10):
        print(f"  {pct(k):6.2f}%  {c}")
    print("in a taker, not waiting at the lock (taker | the frame under it):")
    for (t, c), k in held.most_common(25):
        print(f"  {pct(k):6.2f}%  {t} | {c}")
