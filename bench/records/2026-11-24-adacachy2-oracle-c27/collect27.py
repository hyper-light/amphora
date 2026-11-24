#!/usr/bin/env python3
"""Lane C27: the runs' totals as the record's tables (lane C11's collect.py, the findings by feature from the summaries themselves). usage: collect27.py REC RUN_DIR..."""
import collections, json, os, re, sys
R, runs = sys.argv[1], sys.argv[2:]
os.makedirs(R, exist_ok=True)
S = [(os.path.basename(d), json.load(open(d + "/summary.json"))) for d in runs]
tot = collections.defaultdict(collections.Counter); skip = collections.Counter(); ref = collections.Counter(); feat = collections.defaultdict(collections.Counter); fref = collections.defaultdict(collections.Counter); sref = collections.Counter(); ex = {}; byf = collections.defaultdict(collections.Counter)
print("| run | oracles | databases | checks | findings | more of the same | seconds |\n|---|---|---:|---:|---:|---:|---:|")
for n, s in S:
    ors = [k for k in s["oracles"] if k != "same"]
    print(f"| {n} | {', '.join(ors)} | {s['states']:,} | {sum(o['checks'] for o in s['oracles'].values()):,} | {len(s['findings'])} | {s['duplicates']:,} | {s['seconds']:.0f} |")
    for k, o in s["oracles"].items():
        for x in ("checks", "passed", "failed", "digits", "plans_changed"): tot[k][x] += o[x]
        tot[k]["skipped"] += sum(o["skipped"].values()); tot[k]["refused"] += sum(o["refused"].values())
        for m, c in o["skipped"].items(): skip[(k, m)] += c
        for m, c in o["refused"].items(): ref[m] += c
    for k, f in s["features"].items():
        for x in ("databases", "checks", "passed", "failed", "skipped"): feat[k][x] += f[x]
        feat[k]["refused"] += sum(f["refused"].values())
        for m, c in f["refused"].items(): fref[k][m] += c
    for m, c in s["setup_refused"].items(): sref[m] += c
    for m, v in s.get("setup_examples", {}).items():
        if m not in ex or len(" ".join(v)) < len(" ".join(ex[m])): ex[m] = v
    for f in s["findings"].values():
        for x in f.get("features") or ["(none of them)"]: byf[x][f["kind"]] += 1
    json.dump(s, open(f"{R}/summary-{n}.json", "w"), indent=1)
print(f"\nTOTAL databases {sum(s['states'] for _, s in S):,} checks {sum(t['checks'] for t in tot.values()):,} findings {sum(len(s['findings']) for _, s in S)} (unique {len(set(k for _, s in S for k in s['findings']))})\n")
print("| oracle | checks | passed | of them, digits alone | failed (before dedupe) | skipped | refused | other plan |\n|---|---:|---:|---:|---:|---:|---:|---:|")
for k, t in sorted(tot.items()): print(f"| {k} | {t['checks']:,} | {t['passed']:,} | {t['digits']:,} | {t['failed']:,} | {t['skipped']:,} | {t['refused']:,} | {t['plans_changed']:,} |")
print("\n| oracle | skipped because | count |\n|---|---|---:|")
for (k, m), c in sorted(skip.items(), key=lambda x: (x[0][0], -x[1])): print(f"| {k} | {m} | {c:,} |")
print("\n| feature | databases | checks | passed | failed | skipped | refused | findings: wrong | error | estimate | panic |\n|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|")
for k in sorted(set(feat) | set(byf)):
    t = feat[k]; b = byf[k]
    print(f"| {k} | {t['databases']:,} | {t['checks']:,} | {t['passed']:,} | {t['failed']:,} | {t['skipped']:,} | {t['refused']:,} | {b['Wrong']} | {b['Error']} | {b['Estimate']} | {b['Panic']} |")
val = re.compile(r"JSON|json|too big|overflow|Unexpected|must be|number of arguments|single character|between")
print("\nREFUSALS (not a value's error)")
for m, c in ref.most_common():
    if not val.search(m): print(f"| `{m}` | {c:,} | {', '.join(f'{k} {v[m]}' for k, v in sorted(fref.items(), key=lambda x: -x[1][m])[:4] if v[m])} |")
print("value's errors", f"{sum(c for m, c in ref.items() if val.search(m)):,}", "messages", sum(1 for m in ref if val.search(m)))
print("\nVALUE ERRORS"); [print(f"| `{m}` | {c:,} |") for m, c in ref.most_common(400) if val.search(m)][:0]
for m, c in [(m, c) for m, c in ref.most_common() if val.search(m)][:8]: print(f"| `{m[:110]}` | {c:,} |")
print("\nSETUP REFUSED")
for m, c in sref.most_common(25): print(f"| `{m}` | {c:,} | `{ex.get(m, [''])[-1][:190]}` |")
print("rest", sum(c for m, c in sref.most_common()[25:]))
json.dump({"oracles": {k: dict(v) for k, v in tot.items()}, "features": {k: dict(v) for k, v in feat.items()}, "refused": dict(ref), "setup_refused": dict(sref)}, open(R + "/totals.json", "w"), indent=1)
