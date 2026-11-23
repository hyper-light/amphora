import sys, json, statistics, collections
by = collections.defaultdict(list); firsts = collections.defaultdict(set); status = collections.Counter()
for path in sys.argv[1:]:
    for line in open(path):
        try: r = json.loads(line)
        except Exception: continue
        if r.get("record") == "query":
            key = (r["query"], r["target"], "-")
        elif r.get("record") == "scaling":
            key = (r["query"], r["target"], r["cores"])
        else: continue
        for run in r.get("runs", []):
            status[(key, run.get("status"))] += 1
            if run.get("status") == "done":
                by[key].append(run["ms"]); firsts[key].add(str(run.get("first")))
for key in sorted(by, key=lambda k: (k[0], str(k[2]), k[1])):
    v = by[key]
    print(f"{key[0]} {key[1]:20s} cores={key[2]} n={len(v)} median={statistics.median(v):8.1f} min={min(v):8.1f} max={max(v):8.1f} first={'|'.join(sorted(firsts[key]))[:90]}")
bad = {k: c for k, c in status.items() if k[1] != "done"}
print("non-done runs:", bad if bad else 0)
