import json, sys, re
recs = [json.loads(l) for l in open(sys.argv[1])]
print("| query | engine | timed runs (ms, answer-writing run left out) | min | max | spread % | load at runs |")
print("|---|---|---|---:|---:|---:|---|")
for r in recs:
    if "runs" not in r or "query" not in r: continue
    eng = r.get("target") or r.get("engine")
    cores = r.get("cores")
    runs = r["runs"]
    ms = [x.get("ms") for x in runs if isinstance(x, dict) and x.get("status", "done") == "done" and x.get("ms") is not None]
    loads = [x.get("load") for x in runs if isinstance(x, dict) and x.get("load") is not None]
    if len(sys.argv) > 2 and runs and isinstance(runs[0], dict) and runs[0].get("status", "done") == "done": ms = ms[1:]
    name = f"{eng}@{cores}" if cores else eng
    if not ms:
        print(f"| {r['query']} | {name} | {runs if not isinstance(runs[0], dict) else [x.get('status') for x in runs]} | - | - | - | - |"); continue
    lo, hi = min(ms), max(ms)
    ld = f"{min(loads):.1f}-{max(loads):.1f}" if loads else "-"
    print(f"| {r['query']} | {name} | {' '.join(f'{m:.1f}' for m in ms)} | {lo:.1f} | {hi:.1f} | {100*(hi-lo)/lo:.0f} | {ld} |")
