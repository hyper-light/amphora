#!/usr/bin/env python3
"""Lane C27: every finding file given (or under the run directories given, <run>/findings/*.md) replayed on one build under the four
settings against the oracles' SQLite 3.54.0 (and, when wrong, the system's), both answers kept. usage: replay27.py BENCH LIB SYSLIB JOBS OUT.json PATH..."""
import concurrent.futures as cf, glob, json, os, re, subprocess, sys
bench, lib, syslib, jobs, out, paths = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4]), sys.argv[5], sys.argv[6:]
old = {}
if os.path.exists(out):
    old = {r["file"]: r for r in json.load(open(out))}

def blocks(t):
    return [b.split("```")[0] for b in t.split("```sql\n")[1:]]

def replay(f, l):
    try:
        p = subprocess.run([bench, "--oracle", "--sqlite", l, "--replay", f], capture_output=True, text=True, timeout=90, errors="replace")
        o, e, rc = p.stdout, p.stderr, p.returncode
    except subprocess.TimeoutExpired as x:
        o, e, rc = (x.stdout or b"").decode(errors="replace") if isinstance(x.stdout, bytes) else (x.stdout or ""), "timed out", 124
    settings = {}; cur = None
    for b in re.split(r"\n(?=#### |== )", "\n" + o):
        b = b.strip("\n")
        m = re.match(r"#### (.*?): (.*)", b)
        if m:
            cur = m.group(1); settings[cur] = {"setup": b.splitlines()[0][:600], "q": []}; continue
        m = re.match(r"== (same|DIFFERS) (.*?)\n-- SQLite:  (.*?)\n-- Amphora: (.*?)(?=\n-- Amphora's plan|\Z)(.*)", b, re.S)
        if m and cur is not None:
            pl = re.search(r"-- Amphora's plan: (.*?)(?=\n-- Amphora's plan \(JSON\)|\Z)", m.group(5), re.S)
            settings[cur]["q"].append({"v": m.group(1), "q": m.group(2), "s": m.group(3)[:3000], "a": m.group(4)[:3000], "plan": (pl.group(1) if pl else "")[:1500]})
    pan = re.search(r"panicked at (.*?)\n(.*)", e)
    return settings, rc, (pan.group(1)[-70:] + " :: " + pan.group(2)[:300]) if pan else ("" if rc == 0 else e[-300:])

found = []
for p in paths:
    if os.path.isdir(p): found += sorted(glob.glob(p + "/findings/*.md")) if os.path.isdir(p + "/findings") else sorted(glob.glob(p + "/*.md"))
    else: found.append(p)
seen = set(); uniq = []
for f in found:
    k = os.path.basename(f)
    if k in seen: continue
    seen.add(k); uniq.append(f)

def one(f):
    if f in old and old[f].get("size") == os.path.getsize(f) and "--again" not in paths: return old[f]
    t = open(f, errors="replace").read(); head = (t.splitlines() or [""])[0]; m = re.match(r"# (\S+) (\w+): (.*)", head); b = blocks(t)
    r = {"id": os.path.basename(f)[:-3], "file": f, "size": os.path.getsize(f), "oracle": m.group(1) if m else "", "kind": m.group(2) if m else "?",
         "stmts": b[0].strip() if b else "", "queries": b[1].strip() if len(b) > 1 else "", "diff": t.split("## What differed")[-1].strip().strip("`").strip()[:3000]}
    r["settings"], r["rc"], r["panic"] = replay(f, lib)
    r["differs"] = [k for k, v in r["settings"].items() if any(q["v"] == "DIFFERS" for q in v["q"])]
    if r["differs"] and r["rc"] == 0:
        s2, rc2, _ = replay(f, syslib)
        r["system_differs"] = [k for k, v in s2.items() if any(q["v"] == "DIFFERS" for q in v["q"])]; r["system_rc"] = rc2
    return r

with cf.ThreadPoolExecutor(jobs) as ex: rs = list(ex.map(one, uniq))
json.dump(rs, open(out, "w"), indent=1)
print("replayed", len(rs), "differs", sum(1 for r in rs if r["differs"]), "panics or crashes", sum(1 for r in rs if r["rc"] not in (0,)), "agree with both", sum(1 for r in rs if not r["differs"] and r["rc"] == 0))
