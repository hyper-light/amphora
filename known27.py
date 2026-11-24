#!/usr/bin/env python3
"""Lane C27: lane C11's findings (its replayed rows with C11's causes) as they stand on this build. usage: known27.py REPLAY.json C11_FINDINGS.tsv"""
import collections, json, sys
rs = json.load(open(sys.argv[1])); cause = {}
for l in open(sys.argv[2]):
    p = l.rstrip("\n").split("\t")
    if len(p) > 1: cause[p[1]] = p[0]
by = collections.defaultdict(list)
for r in rs: by[cause.get(r["id"], "?" + r["id"].split("-")[0])].append(r)
for c, v in sorted(by.items(), key=lambda x: -len(x[1])):
    bad = [r for r in v if r["differs"] or r["rc"] != 0]
    sysok = [r for r in bad if r["rc"] == 0 and not r.get("system_differs")]
    open_ = [r for r in bad if r not in sysok]
    print(f"{c}: {len(v)} replayed, {len(v)-len(bad)} answer now, {len(sysok)} reference's own, {len(open_)} still differ: " + ", ".join(f"{r['id']}{'(panic)' if r['rc'] else ''}" for r in open_[:4]))
