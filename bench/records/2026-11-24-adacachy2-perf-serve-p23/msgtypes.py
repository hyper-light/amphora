import sys, os, json, collections
T = {0: "Hup", 1: "Beat", 2: "Propose", 3: "Append", 4: "AppendResp", 5: "Vote", 6: "VoteResp", 7: "Snap", 8: "Heart", 9: "HeartResp", 10: "Unreach", 11: "SnapSt", 12: "ChkQ", 13: "Xfer", 14: "TimeoutNow", 15: "ReadIndex", 16: "ReadIndexResp", 17: "PreVote", 18: "PreVoteResp"}
agg = collections.defaultdict(lambda: collections.defaultdict(list))
for d in sys.argv[1:]:
    name = os.path.basename(d.rstrip("/")); arm, cls = name.split("-")[:2]
    meta = json.load(open(f"{d}/meta.json")); L = meta["leader"]
    t1, t2 = meta["t1"] + 2_000_000_000, meta["t2"] - 1_500_000_000
    span = (t2 - t1) / 1e9
    c = {"L": collections.Counter(), "F": collections.Counter()}
    commits = 0; steps = collections.Counter()
    for m in (1, 2, 3):
        role = "L" if m == L else "F"
        for line in open(f"{d}/m{m}.err", errors="replace"):
            if not line.startswith("P23 "): continue
            p = line.split()
            if len(p) < 8: continue
            t = int(p[1])
            if not (t1 <= t <= t2): continue
            if p[2] == "out": c[role]["out " + T.get(int(p[4]), p[4])] += 1
            if p[2] == "msg": c[role]["in " + T.get(int(p[4]), p[4])] += 1
            if p[2] == "commit" and m == L: commits += 1
            if p[2] in ("long_step", "taken", "handing", "sess_begin", "barrier", "proposed", "slot_start", "tail_wait"): c[role]["ev " + p[2]] += 1
    key = (arm, cls)
    agg[key]["span"].append(span); agg[key]["commits"].append(commits)
    for role in c:
        for k, v in c[role].items(): agg[key][role + " " + k].append(v)
for key in sorted(agg):
    a = agg[key]; tc = sum(a["commits"]); ts = sum(a["span"])
    print(key, "commits/s %.1f" % (tc / ts))
    for k in sorted(a):
        if k in ("span", "commits"): continue
        tot = sum(a[k]); print("   %-28s %7d   per commit %.2f   per s %.1f" % (k, tot, tot / max(tc, 1), tot / ts))
