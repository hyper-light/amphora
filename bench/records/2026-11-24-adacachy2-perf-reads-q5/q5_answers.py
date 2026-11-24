#!/usr/bin/env python3
"""Lane Q5: each session's latest page as Amphora answered it after K0's mixed phase
(AMPHORA_ANSWERS, one JSON line a session) held to SQLite's answer to the same query on the same
rows: the events the bench wrote to each session (ids e-0 to e-<written - 1>, every write answered).
SQLite can't read Amphora's files: its log carries Amphora's own format version.
usage: q5_answers.py TAG ANSWERS"""
import json, sqlite3, sys

tag, answers = sys.argv[1:3]
pages = [json.loads(line) for line in open(answers)]
con = sqlite3.connect(":memory:")
con.execute("CREATE TABLE events(session TEXT NOT NULL, id TEXT NOT NULL, seq INTEGER NOT NULL, PRIMARY KEY (session, id))")
con.executemany("INSERT INTO events VALUES (?1, ?2, ?3)", ((p["session"], f"e-{q}", q) for p in pages for q in range(p["written"])))
differ, first = 0, None
for p in pages:
    ids = [r[0] for r in con.execute("SELECT id FROM events WHERE session = ?1 ORDER BY seq DESC LIMIT 50", (p["session"],))]
    if ids != p["ids"]:
        differ += 1
        first = first or {"session": p["session"], "amphora": p["ids"][:3], "sqlite": ids[:3]}
events = con.execute("SELECT count(*) FROM events").fetchone()[0]
print(json.dumps({"tag": tag, "sessions": len(pages), "differ": differ, "events": events, "first_difference": first}))
