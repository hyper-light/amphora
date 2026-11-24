#!/usr/bin/env python3
"""Lane S9: every read answer of every arm in every round held to SQLite's of the same round, every value as text,
strictly (rows of a statement with no ORDER BY as a multiset): bench/suite_answers.py's rule for two SQLite-dialect
targets. usage: s9_answers.py ROUNDS_DIR   (exit 1 when anything differs, errs or is missing)"""
import glob, importlib.util, json, os, re, sys
spec = importlib.util.spec_from_file_location("suite_answers", os.path.join(os.path.dirname(os.path.abspath(__file__)), "suite_answers.py"))
sa = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sa)
bad = 0
fallback = {}
for p in sorted(glob.glob(os.path.join(sys.argv[1], "round[0-9]*.jsonl"))):
    for line in open(p):
        r = json.loads(line)
        if r["arm"] == "S" and isinstance((r.get("report") or {}).get("answers"), list) and not fallback:
            fallback = {(a["op"], json.dumps(a["params"])): a for a in r["report"]["answers"]}
            print(f"reference when a round has no SQLite pass: SQLite's answers of {os.path.basename(p)} (the data and the parameters drawn are functions of the row ids and the client, the same in every pass)")
for p in sorted(glob.glob(os.path.join(sys.argv[1], "round[0-9]*.jsonl")), key=lambda p: int(re.search(r"round(\d+)", p).group(1))):
    arms = {}
    for line in open(p):
        r = json.loads(line)
        rep = r.get("report")
        if isinstance(rep, dict) and isinstance(rep.get("answers"), list):
            arms[r["arm"]] = rep["answers"]
    ref = {(a["op"], json.dumps(a["params"])): a for a in arms.get("S", [])}
    if not ref:
        ref = fallback
        print(f"{os.path.basename(p)}: no SQLite pass in this round; compared with the reference round's")
    if not ref:
        print(f"{os.path.basename(p)}: no SQLite (arm S) answers to compare with"); bad += 1; continue
    for arm, ans in sorted(arms.items()):
        if arm == "S":
            continue
        if not ans:
            print(f"{os.path.basename(p)} {arm}: writes only, no read answers to compare")
            continue
        compared = differ = missing = 0
        for a in ans:
            r = ref.get((a["op"], json.dumps(a["params"])))
            if r is None:
                missing += 1; continue
            compared += 1
            mine, theirs = a["answer"], r["answer"]
            if "error" in mine or "error" in theirs:
                differ += 1
                print(f"ERROR {os.path.basename(p)} {arm}: {a['op']} {a['sql']} params {a['params']}: {mine.get('error', 'answered')} | sqlite: {theirs.get('error', 'answered')}"); continue
            x, y = mine["rows"], theirs["rows"]
            if not sa.ordered(a["sql"][0] if a["sql"] else ""):
                key = lambda row: json.dumps(row)
                x, y = sorted(x, key=key), sorted(y, key=key)
            if len(x) == len(y) and all(len(m) == len(n) and all(sa.same(u, v, False) for u, v in zip(m, n)) for m, n in zip(x, y)):
                continue
            differ += 1
            print(f"DIFFERS {os.path.basename(p)} {arm} vs sqlite: {a['op']} client {a['client']} sql {a['sql']} params {json.dumps(a['params'])}\n  {arm}: {sa.show(x)[:1500]}\n  sqlite: {sa.show(y)[:1500]}")
        print(f"{os.path.basename(p)} {arm} vs sqlite: {compared} answers compared, {differ} differ or failed, {missing} with no sqlite answer")
        bad += differ + missing
        if not compared:
            bad += 1
sys.exit(1 if bad else 0)
