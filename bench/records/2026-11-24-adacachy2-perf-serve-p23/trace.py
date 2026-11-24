#!/usr/bin/env python3
"""Lane P23: per-stage times of each follower's write, from a traced cell's members' stderr.

    trace.py DIR...      (each DIR a cell23t.py OUT; its name is ARM-CLASS-32-rN)
"""
import bisect, collections, json, os, re, statistics, sys

TYPES = {0: "Hup", 1: "Beat", 2: "Propose", 3: "Append", 4: "AppendResp", 5: "Vote", 6: "VoteResp", 7: "Snap", 8: "Heart", 9: "HeartResp", 15: "ReadIndex", 16: "ReadIndexResp"}


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


def firsts(ev, name, a=None):
    return [e for e in ev if e[1] == name and (a is None or e[2] == a)]


def after(ts, t):
    i = bisect.bisect_left(ts, t)
    return i if i < len(ts) else None


def before(ts, t):
    i = bisect.bisect_right(ts, t) - 1
    return i if i >= 0 else None


def med(v):
    return statistics.median(v) if v else float("nan")


def mean(v):
    return sum(v) / len(v) if v else float("nan")


out = collections.defaultdict(lambda: collections.defaultdict(list))
meta_all = collections.defaultdict(list)
for d in sys.argv[1:]:
    name = os.path.basename(d.rstrip("/"))
    arm, cls = name.split("-")[0], name.split("-")[1]
    meta = json.load(open(f"{d}/meta.json"))
    L = meta["leader"]
    ev = {i: load(f"{d}/m{i}.err") for i in (1, 2, 3)}
    t1 = meta["t1"] + 2_000_000_000  # past the warmup (driver start + about 1 s) and its first second
    t2 = meta["t2"] - 1_500_000_000
    key = (arm, cls)
    meta_all[key].append((meta["commits_s"], meta["p50"]))
    le = ev[L]
    ss = {f: [e for e in le if e[1] == "slot_start" and e[2] == f] for f in (1, 2, 3) if f != L}
    fw = {f: [e for e in le if e[1] == "fwd_in" and e[2] == f] for f in ss}
    ap = {f: [e for e in le if e[1] == "appended" and e[2] == f] for f in ss}
    ssall = [e for e in le if e[1] == "slot_start"]
    ssall_t = [e[0] for e in ssall]
    commits = [e for e in le if e[1] == "commit"]
    commits_idx = [e[2] for e in commits]
    o = out[key]
    for f in ss:
        fe = ev[f]
        by = collections.defaultdict(list)
        for e in fe:
            by[e[1]].append(e)
        ts = {k: [e[0] for e in v] for k, v in by.items()}
        for pr in by["proposed"]:
            t = pr[0]
            if not (t1 <= t <= t2) or pr[2] != 0:
                continue
            def last(k, at=t):
                i = before(ts.get(k, []), at)
                return by[k][i] if i is not None else None
            def nxt(k, at=t):
                i = after(ts.get(k, []), at)
                return by[k][i] if i is not None else None
            go = last("prep_go")
            rs = last("read_state")
            if not go or not rs:
                continue
            # the write's own session: the events between go and proposed
            row = {}
            row["rs>go"] = go[0] - rs[0]
            row["go>prop"] = t - go[0]
            b = lambda k: next((e for e in by[k] if go[0] <= e[0] <= t), None)
            if arm.startswith("H") or arm.startswith("diag-H") or arm == "H":
                hb, hf, hg, wb = b("hand_begin"), b("hand_flushed"), b("handing"), b("w_begin")
                sc, sb, sr, si, so, we = b("s_call"), b("s_begun"), b("s_ran"), b("s_imaged"), b("s_rolled"), b("w_end")
                hd, tk, dc = b("handed"), b("taken"), b("decide")
                seq = [("go>hand_begin", go, hb), ("hand_begin>hand_flushed", hb, hf), ("hand_flushed>handing", hf, hg), ("handing>w_begin", hg, wb),
                       ("w_begin>s_call", wb, sc), ("s_call>s_begun", sc, sb), ("s_begun>s_ran", sb, sr), ("s_ran>s_imaged", sr, si),
                       ("s_imaged>s_rolled", si, so), ("s_rolled>w_end", so, we), ("w_end>taken", we, tk), ("taken>decide", tk, dc), ("decide>proposed", dc, pr)]
            else:
                sb0 = b("sess_begin")
                sc, sb, sr, si, so, se = b("s_call"), b("s_begun"), b("s_ran"), b("s_imaged"), b("s_rolled"), b("sess_end")
                seq = [("go>sess_begin", go, sb0), ("sess_begin>s_call", sb0, sc), ("s_call>s_begun", sc, sb), ("s_begun>s_ran", sb, sr), ("s_ran>s_imaged", sr, si),
                       ("s_imaged>s_rolled", si, so), ("s_rolled>sess_end", so, se), ("sess_end>proposed", se, pr)]
            for nm, x, y in seq:
                if x is not None and y is not None:
                    row[nm] = y[0] - x[0]
            po = next((e for e in by["out"] if e[0] >= t and e[3] == 2 and e[2] == L), None)
            if po:
                row["prop>out"] = po[0] - t
                fin = next((e for e in fw[f] if e[0] >= po[0]), None)
                if fin:
                    row["out>fwd_in"] = fin[0] - po[0]
                    a2 = next((e for e in ap[f] if e[0] >= fin[0]), None)
                    if a2:
                        row["fwd_in>appended"] = a2[0] - fin[0]
                        idx = a2[3]
                        ci = bisect.bisect_left(commits_idx, idx) if commits_idx else 0
                        if ci < len(commits):
                            row["appended>commit"] = commits[ci][0] - a2[0]
                        # the next slot begins
                        ni = after(ssall_t, a2[0])
                        if ni is not None:
                            row["appended>next_ss"] = ssall[ni][0] - a2[0]
            sst = [e[0] for e in ss[f]]
            i = before(sst, go[0])
            if i is not None:
                row["ss>go"] = go[0] - sst[i]
                if po:
                    fin = next((e for e in fw[f] if e[0] >= t), None)
                    if fin:
                        row["ss>fwd_in"] = fin[0] - sst[i]
            for k, v in row.items():
                o[k].append(v / 1e6)
    # cycle: slot_start to slot_start at the leader (any member) within the window
    win = [e for e in ssall if t1 <= e[0] <= t2]
    for x, y in zip(win, win[1:]):
        o["cycle ss>ss"].append((y[0] - x[0]) / 1e6)
    # leader's own writes
    le_by = collections.defaultdict(list)
    for e in le:
        le_by[e[1]].append(e)
    for pr in le_by["proposed"]:
        if pr[2] == 1 and t1 <= pr[0] <= t2:
            go = [e for e in le_by["prep_go"] if e[0] <= pr[0]]
            if go:
                o["LEADER go>prop"].append((pr[0] - go[-1][0]) / 1e6)
    o["long_step ms"].extend([e[2] / 1e3 for e in le if e[1] == "long_step" and t1 <= e[0] <= t2])

for key in sorted(out):
    arm, cls = key
    print(f"=== {arm} {cls}  cells {len(meta_all[key])}  commits/s {[c for c, _ in meta_all[key]]}  p50 {[p for _, p in meta_all[key]]}")
    for k in sorted(out[key], key=lambda k: -mean(out[key][k])):
        v = out[key][k]
        print(f"  {k:28s} n={len(v):5d} mean={mean(v):9.3f} median={med(v):9.3f} p90={sorted(v)[int(len(v) * .9)] if v else 0:9.3f}")
