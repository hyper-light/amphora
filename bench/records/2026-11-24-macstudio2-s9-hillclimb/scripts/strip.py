#!/usr/bin/env python3
"""Lane S9: round*.jsonl without the answers (compared by s9_answers.py), for the record. usage: strip.py IN_DIR OUT_DIR"""
import glob, json, os, sys
for p in sorted(glob.glob(os.path.join(sys.argv[1], "round[0-9]*.jsonl"))):
    with open(os.path.join(sys.argv[2], os.path.basename(p)), "w") as out:
        for line in open(p):
            r = json.loads(line)
            rep = r.get("report")
            if isinstance(rep, dict):
                rep["answers_stripped"] = len(rep.pop("answers", None) or [])
            out.write(json.dumps(r, separators=(",", ":")) + "\n")
