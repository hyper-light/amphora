#!/usr/bin/env python3
"""Lane C11: every finding under the runs given (<run>/findings/*.md), replayed on one build under the four settings against the
oracles' SQLite and the system's, named by its cause, the causes counted and ranked with their smallest example.
usage: rank.py BENCH SQLITE SYSTEM OUT_DIR JOBS RUN_DIR..."""
import concurrent.futures as cf, collections, glob, json, os, re, subprocess, sys
bench, lib, syslib, out, jobs, runs = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4], int(sys.argv[5]), sys.argv[6:]

def blocks(t):
    return [b.split("```")[0] for b in t.split("```sql\n")[1:]]

def replay(f, l):
    try:
        p = subprocess.run([bench, "--oracle", "--sqlite", l, "--replay", f], capture_output=True, text=True, timeout=60)
    except subprocess.TimeoutExpired:
        return {}, "timed out", 124
    s, cur = {}, None
    for line in p.stdout.splitlines():
        m = re.match(r"#### (.*?): ", line)
        if m:
            cur = m.group(1); s[cur] = []
        m = re.match(r"== (same|DIFFERS) ", line)
        if m and cur is not None:
            s[cur].append(m.group(1))
    return s, p.stdout + p.stderr[-400:], p.returncode

# each cause by what its finding shows, the first that fits
ROW_IN = re.compile(r"\(\([^;]*?,[^;]*?\) (NOT )?IN \(SELECT")
def cause(kind, sql, diff):
    t = diff
    if "insufficient registers allocated" in t: return "CM"
    if "row value misused" in t: return "CN"
    if "arg[0] must be a" in t: return "CO"
    if "UPDATE should contain some updated columns" in t: return "CP"
    if "secondary index lacks primary key locator" in t: return "CQ"
    if "InSelect is not supported in this position" in t: return "CR"
    if "Scalar subquery is not supported in this context" in t: return "CS"
    if kind == "Error": return "ERR " + re.sub(r"\d+", "#", t.splitlines()[0] if t else "")[:90]
    if kind == "Panic": return "PANIC " + re.sub(r"\d+", "#", t.splitlines()[0] if t else "")[:90]
    if re.search(r", [^()]*\) (NOT )?IN \(SELECT|\)\) (NOT )?IN \(SELECT|, [^;]{1,160}\) (NOT )?IN \(SELECT s\d+\.c\d+, s\d+", sql): return "CT"
    if "WITH RECURSIVE" in sql and re.search(r"ON \(b1\.c\d (=|IS) w\d\.c0\)", sql): return "CU"
    if "CREATE TRIGGER" in sql and "STRICT" in sql and "BEFORE INSERT" in sql and "FROM lg" in sql: return "CV"
    if "CREATE TRIGGER" in sql: return "TRIGGER"
    return "LOOK"

found = {}
for d in runs:
    for f in glob.glob(d + "/findings/*.md"):
        found.setdefault(os.path.basename(f)[:-3], f)
feats = {}
for d in runs:
    try:
        for k, v in json.load(open(d + "/summary.json"))["findings"].items():
            feats.setdefault(k, v.get("features") or [])
    except Exception as e:
        print("no summary", d, e)

def one(k):
    f = found[k]; t = open(f).read()
    head = t.splitlines()[0]; m = re.match(r"# (\S+) (\w+): (.*)", head)
    b = blocks(t)
    r = {"id": k, "file": f, "oracle": m.group(1) if m else "", "kind": m.group(2) if m else "?", "features": feats.get(k, []),
         "stmts": b[0].strip() if b else "", "queries": b[1].strip() if len(b) > 1 else "", "diff": t.split("## What differed")[-1].strip().strip("`").strip()}
    r["cause"] = cause(r["kind"], r["stmts"] + "\n" + r["queries"], r["diff"])
    s, so, rc = replay(f, lib)
    r["settings"] = s; r["rc"] = rc; r["replay"] = so[-6000:]
    if r["kind"] == "Wrong":
        s2, _, _ = replay(f, syslib)
        r["system"] = s2
        if s and all(x == "same" for v in s2.values() for x in v) and s2:
            r["cause"] = "AH"
        elif s and all(x == "same" for v in s.values() for x in v):
            r["cause"] = "SAME " + r["cause"]
    return r

with cf.ThreadPoolExecutor(jobs) as ex:
    rs = list(ex.map(one, sorted(found)))
os.makedirs(out, exist_ok=True)
json.dump(rs, open(out + "/rank.json", "w"), indent=1)
with open(out + "/findings.tsv", "w") as o:
    for r in sorted(rs, key=lambda r: (r["cause"], len(r["stmts"]) + len(r["queries"]))):
        dif = ",".join(k for k, v in r["settings"].items() if "DIFFERS" in v)
        o.write("\t".join([r["id"], r["kind"], r["cause"], ",".join(r["features"]), dif, str(r["rc"]), " ".join(r["queries"].split())[:300]]) + "\n")
by = collections.defaultdict(list)
for r in rs:
    by[r["cause"]].append(r)
for c, v in sorted(by.items(), key=lambda x: -len(x[1])):
    v.sort(key=lambda r: len(r["stmts"]) + len(r["queries"]))
    orc = collections.Counter(r["oracle"] for r in v)
    st = collections.Counter(",".join(k for k, x in r["settings"].items() if "DIFFERS" in x) or f"rc {r['rc']}" for r in v)
    print(f"\n##### {c}: {len(v)} findings; {dict(orc)}; settings {dict(st.most_common(4))}")
    for r in v[:int(os.environ.get('TOP', '1'))]:
        print("  --", r["id"], r["kind"], r["features"], "rc", r["rc"])
        print("    " + "\n    ".join(x[:500] for x in r["stmts"].splitlines()))
        print("    Q: " + r["queries"][:900].replace("\n", "\n       "))
        print("    " + "\n    ".join(r["diff"].splitlines()[:9])[:600])
print("\nfindings", len(rs), dict(collections.Counter(r["kind"] for r in rs)))
