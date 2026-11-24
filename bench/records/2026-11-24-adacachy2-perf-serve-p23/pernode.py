import json, glob, os, sys, math, statistics, collections
# per-node p50 (leader and followers) and rate, by arm and class, paired by round: H (d0c5722d) over M (6d8e7dcd)
for base in sys.argv[1:]:
    cells = {}
    for f in glob.glob(os.path.join(base, "cells", "*", "rec.jsonl")):
        name = os.path.basename(os.path.dirname(f)); sha, cls, _, r = name.split("-")
        rec = json.loads(open(f).read().strip().splitlines()[-1])
        rows = rec["by_node"]
        cells[(sha, cls, r)] = (rec["writes_per_s"], rec["latency_ms"]["p50"], rec["latency_ms"]["p99"], [(b["latency_ms"]["n"], b["latency_ms"]["p50"]) for b in rows])
    for cls in ("persisted", "replicated"):
        rs = sorted(r for (s, c, r) in cells if s == "d0c5722d" and c == cls and ("6d8e7dcd", cls, r) in cells)
        fol, lead, rate, p50 = [], [], [], []
        for r in rs:
            h, m = cells[("d0c5722d", cls, r)], cells[("6d8e7dcd", cls, r)]
            rate.append(h[0] / m[0]); p50.append(h[1] / m[1])
            # leader is the node with the most writes
            hl = max(range(3), key=lambda i: h[3][i][0]); ml = max(range(3), key=lambda i: m[3][i][0])
            lead.append(h[3][hl][1] / m[3][ml][1])
            hf = statistics.mean(h[3][i][1] for i in range(3) if i != hl); mf = statistics.mean(m[3][i][1] for i in range(3) if i != ml)
            fol.append(hf / mf)
        def gm(v): 
            lr=[math.log(x) for x in v]; m=statistics.mean(lr); s=statistics.stdev(lr)/math.sqrt(len(lr)) if len(lr)>1 else 0
            return "%.2f (%.2f..%.2f)" % (math.exp(m), math.exp(m-2*s), math.exp(m+2*s))
        print(os.path.basename(base), cls, "rounds", len(rs), "H/M commits/s", gm(rate), "| pooled p50", gm(p50), "| leader p50", gm(lead), "| followers p50", gm(fol))
