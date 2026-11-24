#!/usr/bin/env python3
"""Lane C27: every finding under its cause (rules over what its replay showed; the few no rule tells, by hand), the two smallest of each kept beside the record.
usage: sort27.py REPLAY.json REPLAY_FIX.json SELF.json REC"""
import collections, json, os, re, shutil, sys
rs = json.load(open(sys.argv[1])); new = {r["id"]: r for r in json.load(open(sys.argv[2]))}; se = json.load(open(sys.argv[3])); R = sys.argv[4]
HAND = {"diff-13f9ccb207d36fb8": "JP", "diff-281431e9a598ad4c": "DX", "diff-201c6ed49e96e679": "DU", "diff-9723879614d3a7dc": "DV", "diff-ac7bcc8bafa372f3": "DW",
        "diff-b36a3ed9f05d6c81": "CY rowid", "diff-7a39ef6d7ff9f2ff": "CY rowid", "error-327aa6bdaaacbc20": "CJ", "error-9c5ff875d92402a3": "CJ"}
def fold(txt):
    f = []
    for l in txt.split("\n")[1:]:
        l = re.sub(r"'((?:[^']|'')*)'", lambda m: "'" + m.group(1).rstrip(" ").lower() + "'", l.strip()); f.append(re.sub(r"(?<![\w'.])(-?\d+)\.0(?![\d\w])", r"\1", l))
    return sorted(f)
def cause(r):
    q = r["queries"]; i = r["id"]
    if i in HAND: return HAND[i]
    if r["kind"] == "Estimate": return "CL " + r["oracle"]
    if r["rc"] == 0 and r["differs"] and r.get("system_differs") == []: return "AH"
    if any(w.get("order") is False for w in se.get(i, [])): return "RX"
    if r["kind"] == "Error":
        return "C11's CM" if "insufficient registers" in r["diff"] else "C11's CN" if "row value misused" in r["diff"] else "CW"
    n = new[i]; d = [x for v in n["settings"].values() for x in v["q"] if x["v"] == "DIFFERS"]
    if re.search(r"UNION|EXCEPT|INTERSECT", q + r["stmts"]) and "OVER" not in q.replace("OVERFLOW", "") and (not d or all(fold(x["s"]) == fold(x["a"]) for x in d)): return "CY"
    if not d: return "CY"
    if "OVER" in q and "GROUP BY" in q: return "RW"
    if "json_pretty" in q: return "CI"
    if re.search(r"jsonb_|jsonb\(", q + r["stmts"]): return "CK (JSONB)"
    if re.search(r"octet_length|length\(|replace\(|\|\||instr|quote\(", q): return "CK (inside)"
    return "LOOK"
by = collections.defaultdict(list)
for r in rs: r["cause"] = cause(r); by[r["cause"]].append(r)
os.makedirs(R + "/findings", exist_ok=True)
for c, v in sorted(by.items(), key=lambda x: -len(x[1])):
    v.sort(key=lambda r: len(r["stmts"]) + len(r["queries"])); o = collections.Counter(r["oracle"] for r in v); st = collections.Counter(",".join(r["differs"]) or "none" for r in v); ft = collections.Counter(r["kind"] for r in v)
    print(f"{c}: {len(v)} {dict(o)} {dict(ft)} {dict(st.most_common(3))}  smallest {v[0]['id']}")
    for r in v[:2]: shutil.copy(r["file"], f"{R}/findings/{re.sub(r'[^A-Za-z0-9]+', '-', c).strip('-')}-{r['id']}.md")
with open(R + "/findings.tsv", "w") as o:
    for r in sorted(rs, key=lambda r: (r["cause"], len(r["stmts"]) + len(r["queries"]))):
        q = [x for x in r["settings"].get("default", {}).get("q", []) if x["v"] == "DIFFERS"][:1]
        a = " | ".join((" / ".join(y.strip() for y in x.split("\n")))[:110] for x in ((q[0]["s"], q[0]["a"]) if q else ("", "")))
        o.write("\t".join([r["cause"], r["id"], r["kind"], ",".join(r["differs"]) or "none", " ".join((r["queries"] or r["stmts"]).split())[:200 if r["cause"] != "RX" else 130], a if r["cause"] != "RX" else a[:60]]) + "\n")
json.dump({k: len(v) for k, v in sorted(by.items(), key=lambda x: -len(x[1]))}, open(R + "/causes.json", "w"), indent=1)
for r in by.get("LOOK", []): print("LOOK", r["id"], " ".join(r["queries"].split())[:300])
