import json, sys
for line in open(sys.argv[1]):
    r = json.loads(line)
    rep = r.get("report") or {}
    if r.get("failed"):
        print("FAILED", r.get("target"), str(r.get("error"))[-400:])
        continue
    res = rep.get("results") or []
    bad = [(x["op"], x["clients"], x.get("error")) for x in res if x.get("errors") or x.get("unsupported")]
    print(rep.get("target"), "results", len(res), "errors or unsupported:", bad[:8], "rss_max_mb", rep.get("rss_max_mb"))
