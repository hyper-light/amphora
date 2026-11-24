#!/usr/bin/env python3
"""Lane Q5: each session's latest page as Amphora answered it after K0's mixed phase
(AMPHORA_ANSWERS, one JSON line a session) held to SQLite's on the same database and log.
usage: q5_answers.py TAG ANSWERS DB"""
import json, sqlite3, sys

tag, answers, db = sys.argv[1:4]
con = sqlite3.connect(db)
n = differ = 0
first = None
for line in open(answers):
    a = json.loads(line)
    ids = [r[0] for r in con.execute("SELECT id FROM events WHERE session = ?1 ORDER BY seq DESC LIMIT 50", (a["session"],))]
    n += 1
    if ids != a["ids"]:
        differ += 1
        first = first or {"session": a["session"], "amphora": a["ids"][:3], "sqlite": ids[:3]}
events = con.execute("SELECT count(*) FROM events").fetchone()[0]
print(json.dumps({"tag": tag, "sessions": n, "differ": differ, "events": events, "first_difference": first}))
