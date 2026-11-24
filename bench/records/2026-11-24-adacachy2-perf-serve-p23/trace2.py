#!/usr/bin/env python3
"""Lane P23: the same write inline on its owner and on the session worker, alternated write by write within one run (diag arm
H2, P23_ALT=1): per-stage medians by mode, and the leader's slot-to-slot cycle by the holder's mode.

    trace2.py DIR...      (each DIR a cell23t.py OUT; its name is H2-CLASS-32-rN)
"""
import bisect, collections, json, os, statistics, sys


def load(path):
    ev = []
    with open(path, errors="replace") as f:
        for line in f:
            if not line.startswith("P23 "):
                continue
            p = line.split()
            if len(p) < 8:
                continue
            try:
                ev.append((int(p[1]), p[2], int(p[3]), int(p[4]), int(p[5]), int(p[6]), int(p[7])))
            except ValueError:
                pass
    ev.sort()
    return ev


def idx_before(ts, t):
    return bisect.bisect_right(ts, t) - 1


st = collections.defaultdict(lambda: collections.defaultdict(list))
meta_all = collections.defaultdict(list)
for d in sys.argv[1:]:
    name = os.path.basename(d.rstrip("/"))
    arm, cls = name.split("-")[0], name.split("-")[1]
    meta = json.load(open(f"{d}/meta.json"))
    L = meta["leader"]
    ev = {i: load(f"{d}/m{i}.err") for i in (1, 2, 3)}
    t1, t2 = meta["t1"] + 2_000_000_000, meta["t2"] - 1_500_000_000
    meta_all[cls].append((meta["commits_s"], meta["p50"]))
    le = ev[L]
    sslist = [(e[0], e[2]) for e in le if e[1] == "slot_start"]
    sst = [x[0] for x in sslist]
    fwd = {f: [e for e in le if e[1] == "fwd_in" and e[2] == f] for f in (1, 2, 3)}
    app = {f: [e for e in le if e[1] == "appended" and e[2] == f] for f in (1, 2, 3)}
    commits = [e for e in le if e[1] == "commit"]
    cidx = [e[2] for e in commits]
    writes = {}
    for f in (1, 2, 3):
        by = collections.defaultdict(list)
        for e in ev[f]:
            by[e[1]].append(e)
        ts = {k: [x[0] for x in v] for k, v in by.items()}
        rows = []
        for pr in by["proposed"]:
            if (pr[2] == 1) != (f == L):
                continue
            t = pr[0]
            if not (t1 <= t <= t2):
                continue
            cand = []
            for k, mode in (("hand_begin", "worker"), ("sess_begin", "inline")):
                i = idx_before(ts.get(k, []), t)
                if i >= 0:
                    cand.append((by[k][i][0], mode, by[k][i]))
            if not cand:
                continue
            g, mode, go = max(cand)
            rows.append((t, g, mode))
            r = {"go>prop": t - g}
            b = lambda k: next((e for e in by[k] if g <= e[0] <= t), None)
            if mode == "worker":
                for a_, b_, nm in (("handing", "w_begin", "handing>w_begin"), ("w_end", "taken", "w_end>taken"), ("hand_begin", "hand_flushed", "hand_begin>hand_flushed"), ("taken", "decide", "taken>decide"), ("decide", "proposed", "decide>proposed"), ("w_begin", "s_begun", "w_begin>s_begun"), ("s_begun", "s_ran", "s_begun>s_ran"), ("s_ran", "w_end", "s_ran>w_end")):
                    x, y = b(a_), b(b_) if b_ != "proposed" else pr
                    if x is not None and y is not None:
                        r[nm] = y[0] - x[0]
            else:
                for a_, b_, nm in (("s_call", "s_begun", "s_call>s_begun"), ("s_begun", "s_ran", "s_begun>s_ran"), ("s_ran", "s_rolled", "s_ran>s_rolled"), ("sess_end", "proposed", "sess_end>proposed")):
                    x, y = b(a_), b(b_) if b_ != "proposed" else pr
                    if x is not None and y is not None:
                        r[nm] = y[0] - x[0]
            po = next((e for e in by["out"] if e[0] >= t and e[3] == 2 and e[2] == L), None) if f != L else None
            if po:
                r["prop>out"] = po[0] - t
                fi = next((e for e in fwd[f] if e[0] >= po[0]), None)
                if fi:
                    r["go>fwd_in"] = fi[0] - g
                    a2 = next((e for e in app[f] if e[0] >= fi[0]), None)
                    if a2:
                        r["fwd_in>appended"] = a2[0] - fi[0]
                        ci = bisect.bisect_left(cidx, a2[3])
                        if ci < len(commits):
                            r["appended>commit"] = commits[ci][0] - a2[0]
            key = ("leader" if f == L else "follower", cls, mode)
            for k, v in r.items():
                st[key][k].append(v / 1e6)
        writes[f] = rows
    # the leader's cycle, slot to slot, by the holder's mode and by the mode of the leader's own write between
    for i in range(len(sslist) - 1):
        t0, f = sslist[i]
        t3 = sslist[i + 1][0]
        if not (t1 <= t0 and t3 <= t2) or f == L:
            continue
        fm = [m for (t, g, m) in writes[f] if t0 <= g <= t3]
        lm = [m for (t, g, m) in writes[L] if t0 <= g <= t3]
        if len(fm) == 1:
            st[("cycle", cls, "holder " + fm[0])]["ss>ss"].append((t3 - t0) / 1e6)
            if len(lm) == 1:
                st[("cycle", cls, f"holder {fm[0]}, leader {lm[0]}")]["ss>ss"].append((t3 - t0) / 1e6)
        if len(lm) == 1:
            st[("cycle", cls, "leader " + lm[0])]["ss>ss"].append((t3 - t0) / 1e6)

for cls in sorted(meta_all):
    print("===", cls, "cells", len(meta_all[cls]), "commits/s", [c for c, _ in meta_all[cls]], "p50", [p for _, p in meta_all[cls]])
for key in sorted(st):
    print("---", key)
    for k in sorted(st[key], key=lambda k: -statistics.mean(st[key][k])):
        v = sorted(st[key][k])
        n = len(v)
        sd = statistics.stdev(v) if n > 1 else 0.0
        print(f"    {k:26s} n={n:5d} mean={statistics.mean(v):8.3f} +-{sd / max(n, 1) ** .5:6.3f} median={statistics.median(v):8.3f} p90={v[int(n * .9)]:8.3f} max={v[-1]:9.3f}")
