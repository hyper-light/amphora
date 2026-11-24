#!/usr/bin/env python3
"""Lane S7: load and pressure over each phase of the run (rounds.txt windows, phases.log samples every 5 s).

usage: phases_summary.py RUN_OUT_DIR
"""
import re
import statistics
import sys

d = sys.argv[1]
windows = []   # (name, start, end, order)
open_w = {}
for line in open(f"{d}/rounds.txt"):
    m = re.match(r"(smoke-start|smoke-end|round \d+ start|round \d+ end|done)(?: \(([^)]*)\))? (\S+Z) load", line)
    if not m:
        continue
    tag, order, ts = m.group(1), m.group(2), m.group(3)
    if tag == "smoke-start":
        open_w["smoke"] = (ts, "")
    elif tag == "smoke-end":
        s, o = open_w.pop("smoke")
        windows.append(("smoke", s, ts, o))
    elif tag.endswith(" start"):
        open_w[tag[:-6]] = (ts, order or "")
    elif tag.endswith(" end"):
        s, o = open_w.pop(tag[:-4])
        windows.append((tag[:-4], s, ts, o))
samples = []
for line in open(f"{d}/phases.log"):
    m = re.match(r"(\S+Z) \w+ load=([\d.]+) ([\d.]+) ([\d.]+) cpu\[some avg10=([\d.]+) .*?\] io_some\[some avg10=([\d.]+) .*?\] io_full\[full avg10=([\d.]+)", line)
    if m:
        samples.append((m.group(1), float(m.group(2)), float(m.group(5)), float(m.group(6)), float(m.group(7))))


def med(xs):
    return statistics.median(xs) if xs else float("nan")


def span(xs):
    return f"{med(xs):.1f} ({min(xs):.1f}-{max(xs):.1f})" if xs else "-"


print("| phase | order | start | end | samples | load1 median (range) | CPU pressure some avg10 % | IO some avg10 % | IO full avg10 % |")
print("|---|---|---|---|---|---|---|---|---|")
for name, s, e, o in windows:
    inside = [x for x in samples if s <= x[0] <= e]
    print(f"| {name} | {o} | {s} | {e} | {len(inside)} | {span([x[1] for x in inside])} | {span([x[2] for x in inside])} | {span([x[3] for x in inside])} | {span([x[4] for x in inside])} |")
allin = [x for x in samples if windows and windows[0][1] <= x[0] <= windows[-1][2]]
print(f"\nWhole run: {len(allin)} samples, load1 {span([x[1] for x in allin])}, CPU pressure {span([x[2] for x in allin])}, IO some {span([x[3] for x in allin])}, IO full {span([x[4] for x in allin])}.")
