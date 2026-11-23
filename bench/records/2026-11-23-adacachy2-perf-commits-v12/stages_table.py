#!/usr/bin/env python3
"""Lane V12: where each write's time goes, from trace.py's folder. Joins the three members' stage
logs (diag/commits-v12-stages) by the write's context and its entry's index, adds the log's write
and flush syscalls from lane W2's shim, and prints each stage's median, p10 and p90 in ms.

    stages_table.py OUT
"""
import collections, json, os, re, statistics, struct, sys

out = sys.argv[1]
ev = {}  # member -> list of (t, name, dict)
for m in (1, 2, 3):
    rows = []
    for line in open(f"{out}/stages-m{m}.log"):
        parts = line.split()
        if len(parts) < 2:
            continue
        kv = dict(p.split("=", 1) for p in parts[2:] if "=" in p)
        rows.append((int(parts[0]), parts[1], kv))
    ev[m] = rows
rec = json.loads(open(f"{out}/rec.jsonl").read().strip().splitlines()[-1])
# The leader: the member whose drive lines say L most often in the window.
roles = {m: collections.Counter(kv.get("role") for _, n, kv in ev[m] if n == "drive") for m in ev}
leader = max(ev, key=lambda m: roles[m]["L"])
# Each member's first time its log's last, persisted, committed and applied index reached i.
def firsts(m, key):
    pts, top = [], 0
    for t, n, kv in ev[m]:
        if n == "drive":
            v = int(kv[key])
            if v > top:
                pts.append((v, t))
                top = v
    return pts
def at(pts, idx):
    for v, t in pts:
        if v >= idx:
            return t
    return None
reach = {(m, k): firsts(m, k) for m in ev for k in ("last", "persisted", "committed", "applied")}
idx_of = {}
for m in ev:
    for t, n, kv in ev[m]:
        if n == "applied" and kv.get("ok") == "true":
            idx_of.setdefault(kv["ctx"], int(kv["idx"]))
writes = []
for m in ev:
    sub = {}
    first = {}
    for t, n, kv in ev[m]:
        if n == "submit":
            sub.setdefault(kv["until"], t)
        elif n in ("barrier", "prep0", "prep1", "proposed", "answer", "answer_commit"):
            c = kv["ctx"]
            first.setdefault((c, n), t)
            if n == "barrier":
                first.setdefault((c, "until"), kv["until"])
                first.setdefault((c, "w"), kv.get("w"))
    for (c, n), t in list(first.items()):
        if n != "proposed" or c not in idx_of:
            continue
        i = idx_of[c]
        w = {"origin": m, "ctx": c, "idx": i, "submit": sub.get(first.get((c, "until"))), "barrier": first.get((c, "barrier")),
             "prep0": first.get((c, "prep0")), "prep1": first.get((c, "prep1")), "proposed": t,
             "answer": min(x for x in (first.get((c, "answer")), first.get((c, "answer_commit"))) if x is not None) if (first.get((c, "answer")) or first.get((c, "answer_commit"))) else None}
        w["L_last"] = at(reach[(leader, "last")], i)
        w["L_pers"] = at(reach[(leader, "persisted")], i)
        w["L_commit"] = at(reach[(leader, "committed")], i)
        fol = [f for f in ev if f != leader]
        fl = {f: (at(reach[(f, "last")], i), at(reach[(f, "persisted")], i)) for f in fol}
        done = [(p, l, f) for f, (l, p) in fl.items() if p is not None and l is not None]
        if done:
            p, l, f = min(done)
            w["F_last"], w["F_pers"], w["F_first"] = l, p, f
        w["O_commit"] = at(reach[(m, "committed")], i)
        w["O_applied"] = at(reach[(m, "applied")], i)
        writes.append(w)
# The measured window only (the driver's warmup is its first second): writes whose submit is past
# the first second of the run's first submit.
t_first = min((w["submit"] for w in writes if w.get("submit")), default=0)
writes = [w for w in writes if w.get("submit") and w["submit"] >= t_first + 1_000_000_000]
def d(w, a, b):
    if w.get(a) is None or w.get(b) is None:
        return None
    return (w[b] - w[a]) / 1e6
STAGES = [
    ("request to barrier sent (queue at the member)", "submit", "barrier"),
    ("slot wait: barrier sent to prepare start", "barrier", "prep0"),
    ("prepare (the SQL session, prepare_db)", "prep0", "prep1"),
    ("propose: prepare end to proposed", "prep1", "proposed"),
    ("proposed to the leader's log holds it (forward hop for a follower)", "proposed", "L_last"),
    ("leader append + fsync: its log holds it to persisted", "L_last", "L_pers"),
    ("follower append: leader's log holds it to the first follower's log holds it", "L_last", "F_last"),
    ("follower fsync: that follower's log holds it to persisted", "F_last", "F_pers"),
    ("commit: leader's log holds it to the leader's commit covers it", "L_last", "L_commit"),
    ("commit to the origin member knows it committed", "L_commit", "O_commit"),
    ("apply: origin knows it committed to origin applied", "O_commit", "O_applied"),
    ("answer: origin applied to answer sent", "O_applied", "answer"),
    ("total in the member: request to answer sent", "submit", "answer"),
]
by_origin = collections.Counter("leader" if w["origin"] == leader else "follower" for w in writes)
print(f"## {os.path.basename(out)}: {rec.get('writers')} writers, route {rec.get('route')}, leader member {leader}; "
      f"driver: {rec.get('writes_per_s')} commits/s, p50 {rec.get('latency_ms', {}).get('p50')} ms, p99 {rec.get('latency_ms', {}).get('p99')} ms, "
      f"misses {rec.get('deadline_misses')}; traced writes {len(writes)} ({dict(by_origin)})")
for origin in ("all", "leader", "follower"):
    ws = [w for w in writes if origin == "all" or (origin == "leader") == (w["origin"] == leader)]
    if not ws:
        continue
    print(f"\n| stage ({origin}-origin writes, n={len(ws)}) | median ms | p10 ms | p90 ms | n |")
    print("|---|---|---|---|---|")
    for name, a, b in STAGES:
        xs = sorted(x for x in (d(w, a, b) for w in ws) if x is not None)
        if not xs:
            print(f"| {name} | - | - | - | 0 |")
            continue
        q = lambda p: xs[min(len(xs) - 1, int(p * len(xs)))]
        print(f"| {name} | {statistics.median(xs):.3f} | {q(0.1):.3f} | {q(0.9):.3f} | {len(xs)} |")
# Syscalls (lane W2's shim): each member's log writes and flushes in the measured window, by file.
KIND = {1: "pwrite", 2: "write", 3: "pwritev", 4: "fdatasync", 5: "fsync", 6: "open", 7: "close", 8: "futex", 9: "sync_file_range"}
lo = t_first + 1_000_000_000
hi = max((w["answer"] for w in writes if w.get("answer")), default=lo)
print("\n| member | role | file tail | syscall | count/s | median ms | p90 ms |")
print("|---|---|---|---|---|---|---|")
for m in (1, 2, 3):
    path = f"{out}/shim-m{m}.rec"
    if not os.path.exists(path):
        print(f"| {m} | - | (no shim records) | | | | |")
        continue
    data = open(path, "rb").read()
    groups = collections.defaultdict(list)
    for off in range(0, len(data) - 55, 56):
        t0, t1, ret, a, b, tid, kind, tail = struct.unpack_from("<QQqqqih10s", data, off)
        if kind in (1, 2, 3, 4, 5, 9) and lo <= t0 <= hi:
            name = tail.rstrip(b"\0").decode("latin-1")
            if "stages" in name:
                continue
            groups[(name, KIND.get(kind, str(kind)))].append((t1 - t0) / 1e6)
    span = max((hi - lo) / 1e9, 1e-9)
    for (name, k), xs in sorted(groups.items(), key=lambda kv: -len(kv[1]))[:6]:
        xs.sort()
        print(f"| {m} | {'leader' if m == leader else 'follower'} | {name!r} | {k} | {len(xs) / span:.1f} | {statistics.median(xs):.3f} | {xs[min(len(xs) - 1, int(0.9 * len(xs)))]:.3f} |")
