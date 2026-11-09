#!/usr/bin/env python3
"""The SQL parser's share of an `amphora-bench --sql-api` shape's loop, from the .folded profile
AMPHORA_PROFILE writes (main.rs, write_profile: one stack a line, root first, then its samples).

A shape's loop is every sample whose stack holds its run function (sqlapi::reapi_run,
logsearch_run or insert_run, the closures it hands Database::call among them). Of those, the
parser's are the samples with a frame in Amphora's SQL parser (amphora_engine_parser::) or in the
session's validation (amphora::sql::validate): the session's parse and the engine's own prepare
both count.

usage: python3 bench/prof_share.py <prefix>-<shape>-<run>.folded ...
  prints one JSON line a file: its shape, the loop's samples, the parser's, and the share.
"""
import json
import re
import sys

PARSER = re.compile(r"amphora_engine_parser::|amphora::sql::validate")
for path in sys.argv[1:]:
    shape = re.search(r"-(reapi|logsearch|insert)-\d+\.folded$", path)
    if not shape:
        sys.exit(f"{path}: not a <prefix>-<shape>-<run>.folded file")
    loop = re.compile(rf"sqlapi::{shape.group(1)}_run")
    total = parser = 0
    for line in open(path):
        stack, _, count = line.rstrip("\n").rpartition(" ")
        if not loop.search(stack):
            continue
        total += int(count)
        if PARSER.search(stack):
            parser += int(count)
    print(json.dumps({"file": path, "shape": shape.group(1), "loop_samples": total,
                      "parser_samples": parser, "parser_share": parser / total if total else None}))
