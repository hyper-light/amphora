#!/usr/bin/env python3
"""Lane S7: the record, assembled from its parts on the machine where the data is.

usage: record_assemble.py OUT_DIR SCRIPTS_DIR END_TIME RECORD_OUT
  OUT_DIR: the run's out directory (TABLES.md, PHASES.md, roundN-answers.txt, smoke-answers.txt, rounds.txt)
  SCRIPTS_DIR: record-head.md, record-tail.md, load-template.md, judgment.md, verdict.py
"""
import glob
import os
import re
import subprocess
import sys

out, scripts, end, dest = sys.argv[1:5]
read = lambda *p: open(os.path.join(*p)).read()
verdict = subprocess.run([sys.executable, "-I", os.path.join(scripts, "verdict.py"), os.path.join(out, "TABLES.md")],
                         capture_output=True, text=True, check=True).stdout.strip()
judgment = read(scripts, "judgment.md").strip() if os.path.exists(os.path.join(scripts, "judgment.md")) else ""
answers = []
for f in ["smoke-answers.txt"] + [f"round{i}-answers.txt" for i in range(1, 20)]:
    p = os.path.join(out, f)
    if os.path.exists(p):
        ok = [l.strip() for l in open(p) if "answers compared" in l]
        rc = [l.strip() for l in open(p) if l.startswith("suite_answers rc=")]
        answers.append(f"{f[:-len('-answers.txt')]}: {'; '.join(ok)} ({'; '.join(rc)})")
phases = "\n".join(l for l in read(out, "PHASES.md").splitlines() if l.strip())
load = read(scripts, "load-template.md").replace("TODO-PHASES", phases).strip()
text = read(scripts, "record-head.md")
text = text.replace("TODO-END", end).replace("TODO-VERDICT", (verdict + ("\n\n" + judgment if judgment else "")).strip())
text = text.replace("TODO-ANSWERS", "\n  - " + "\n  - ".join(answers)).replace("TODO-LOAD", load)
assert "TODO-" not in text, "a TODO mark is left: " + text[text.index("TODO-") - 60:text.index("TODO-") + 60]
tables = read(out, "TABLES.md")
body = tables[tables.index("## 2."):tables.index("## 4.")].rstrip()
tail = read(scripts, "record-tail.md")
open(dest, "w").write(text.rstrip() + "\n\n" + body + "\n\n" + tail)
print(f"record: {len(text.splitlines()) + len(body.splitlines()) + len(tail.splitlines())} lines")
