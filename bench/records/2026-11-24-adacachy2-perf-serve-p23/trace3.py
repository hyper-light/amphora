#!/usr/bin/env python3
"""Lane P23: where a follower's forward waits for its own log write: for each follower's write (diag arm H2), the log write in
flight when it was proposed and where that write began, by mode (worker or inline).

    trace3.py DIR...
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
                ev.append((int(p[1]), p[2], int(p[3]), int(p[4])))
            except ValueError:
                pass
    ev.sort()
    return ev


acc = collections.defaultdict(lambda: collections.defaultdict(list))
cnt = collections.defaultdict(collections.Counter)
for d in sys.argv[1:]:
    name = os.path.basename(d.rstrip("/"))
    arm, cls = name.split("-")[0], name.split("-")[1]
    meta = json.load(open(f"{d}/meta.json"))
    L = meta["leader"]
    t1, t2 = meta["t1"] + 2_000_000_000, meta["t2"] - 1_500_000_000
    for f in (1, 2, 3):
        if f == L:
            continue
        ev = load(f"{d}/m{f}.err")
        starts, dones = {}, {}
        prior = []  # open writes by number
        evs = [e for e in ev if e[1] in ("wstart", "wdone", "proposed", "out", "hand_begin", "sess_begin", "handing", "taken", "read_state", "barrier")]
        writes = [(e[0], e[1], e[2], e[3]) for e in evs if e[1] in ("wstart", "wdone")]
        wts = [w[0] for w in writes]
        ws = [w for w in writes if w[1] == "wstart"]
        wd = [w for w in writes if w[1] == "wdone"]
        dn = {w[2]: w[0] for w in wd}
        gos = [(e[0], "worker" if e[1] == "hand_begin" else "inline") for e in evs if e[1] in ("hand_begin", "sess_begin")]
        gts = [g[0] for g in gos]
        outs = [e[0] for e in ev if e[1] == "out" and e[3] == 2 and e[2] == L]
        for pr in [e for e in evs if e[1] == "proposed" and e[2] == 0]:
            t = pr[0]
            if not (t1 <= t <= t2):
                continue
            i = bisect.bisect_right(gts, t) - 1
            if i < 0:
                continue
            g, mode = gos[i]
            key = (cls, mode)
            j = bisect.bisect_left(outs, t)
            if j >= len(outs):
                continue
            out = outs[j]
            # the write in flight at the proposal: the last wstart before it whose wdone is after it
            fl = [w for w in ws if w[0] <= t and dn.get(w[2], 1 << 62) > t]
            acc[key]["prop>out ms"].append((out - t) / 1e6)
            cnt[key]["n"] += 1
            if not fl:
                cnt[key]["no write in flight at the proposal"] += 1
                # writes begun after the proposal, before out
                nb = [w for w in ws if t < w[0] <= out]
                cnt[key]["... then a write begun before out"] += 1 if nb else 0
                acc[key]["prop>out, no write in flight ms"].append((out - t) / 1e6)
                continue
            w = fl[-1]
            acc[key]["prop>out, a write in flight ms"].append((out - t) / 1e6)
            acc[key]["age of the write in flight at the proposal ms"].append((t - w[0]) / 1e6)
            acc[key]["age: begun during the session (after its go) ms"].append((w[0] - g) / 1e6 if w[0] >= g else float("nan"))
            cnt[key]["write in flight at the proposal"] += 1
            if w[0] >= g:
                cnt[key]["... begun after the session began (its go)"] += 1
                acc[key]["prop>out, in-flight write begun after go ms"].append((out - t) / 1e6)
            else:
                cnt[key]["... begun before the session began"] += 1
                acc[key]["prop>out, in-flight write begun before go ms"].append((out - t) / 1e6)
            # does the forward leave when that write ends, or a later one?
            end = dn.get(w[2])
            if end is not None:
                acc[key]["out - end of the in-flight write ms"].append((out - end) / 1e6)
                # how many writes ended between the proposal and out
                acc[key]["writes ended between proposal and out"].append(sum(1 for x in wd if t < x[0] <= out))
                acc[key]["writes started between proposal and out"].append(sum(1 for x in ws if t < x[0] <= out))
            acc[key]["in-flight write carries entries (b>=1000, msgs)"].append(float(w[3] % 1000))
for key in sorted(acc):
    print("===", key, dict(cnt[key]))
    for k in sorted(acc[key]):
        v = [x for x in acc[key][k] if x == x]
        if not v:
            continue
        v.sort()
        print(f"    {k:60s} n={len(v):4d} mean={statistics.mean(v):8.3f} median={statistics.median(v):8.3f} p90={v[int(len(v) * .9)]:8.3f}")
