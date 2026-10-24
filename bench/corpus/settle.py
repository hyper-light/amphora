#!/usr/bin/env python3
"""Settling a translated source's files (G2 step 2): a query whose answer changed between the run
that wrote it and its replay (build_corpus.py replay-<source>) answers from the clock or chance
through its data (PostgreSQL's timestamp tests store 'now', which SQLite's date functions read as
the time), so its record becomes a statement: run, its answer not checked. Replay again after.

usage: settle.py <corpus dir> <source>
  reads <corpus dir>/replay-<source>.json, rewrites the records it names in <corpus dir>/<source>/,
  and writes what it settled to <corpus dir>/settled-<source>.json
"""
import collections
import json
import os
import sys

CORPUS, SOURCE = sys.argv[1:3]


def main():
    report = json.load(open(os.path.join(CORPUS, f"replay-{SOURCE}.json")))
    lines_of = collections.defaultdict(set)
    for rel, f in report["files"].items():
        for d in f["first_disagreements"]:
            if d[1] == "query":
                lines_of[rel].add(d[0])
    settled = 0
    for rel, lines in lines_of.items():
        path = os.path.join(CORPUS, rel)
        text = open(path).read().split("\n")
        out, i = [], 0
        while i < len(text):
            if i + 1 in lines and text[i].startswith("query "):
                out.append("statement ok")
                i += 1
                while i < len(text) and text[i] != "----" and text[i].strip():
                    out.append(text[i])
                    i += 1
                while i < len(text) and text[i].strip():  # the ---- and the answer
                    i += 1
                settled += 1
                continue
            out.append(text[i])
            i += 1
        open(path, "w").write("\n".join(out))
    json.dump({"source": SOURCE, "settled": settled, "records": {rel: sorted(lines) for rel, lines in lines_of.items()}},
              open(os.path.join(CORPUS, f"settled-{SOURCE}.json"), "w"), indent=1)
    print(f"{SOURCE}: {settled} queries settled into statements in {len(lines_of)} files")


main()
