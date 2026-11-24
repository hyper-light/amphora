#!/usr/bin/env python3
"""Lane Q6: while PID runs, keeps each of its threads' latest scheduler counters (schedstat: ns on a
CPU, ns runnable but waiting for a CPU, timeslices; status: voluntary and involuntary switches),
read every 0.2 s, then prints them summed by thread name. Time runnable-but-waiting beside time on
a CPU, and involuntary switches, say how often the threads (lock holders among them) were taken
off a CPU while they could run. A thread's last 0.2 s before it exits can be missed.
usage: q6_sched.py PID"""
import collections, os, re, sys, time

pid = sys.argv[1]
latest = {}
while os.path.exists(f"/proc/{pid}"):
    try:
        tids = os.listdir(f"/proc/{pid}/task")
    except OSError:
        break
    for tid in tids:
        try:
            with open(f"/proc/{pid}/task/{tid}/schedstat") as f:
                run, wait, slices = map(int, f.read().split())
            with open(f"/proc/{pid}/task/{tid}/comm") as f:
                comm = f.read().strip()
            with open(f"/proc/{pid}/task/{tid}/status") as f:
                st = f.read()
            vol = int(re.search(r"^voluntary_ctxt_switches:\s+(\d+)", st, re.M).group(1))
            invol = int(re.search(r"^nonvoluntary_ctxt_switches:\s+(\d+)", st, re.M).group(1))
            latest[tid] = (comm, run, wait, slices, vol, invol)
        except (OSError, AttributeError, ValueError):
            pass
    time.sleep(0.2)

groups = collections.defaultdict(lambda: [0, 0, 0, 0, 0, 0])
for comm, run, wait, slices, vol, invol in latest.values():
    g = groups[re.sub(r"\d+", "#", comm)]
    for i, v in enumerate((1, run, wait, slices, vol, invol)):
        g[i] += v
print("thread | threads | cpu s | runnable-waiting s | waiting/cpu | timeslices | voluntary | involuntary")
for name, (k, run, wait, slices, vol, invol) in sorted(groups.items(), key=lambda kv: -kv[1][1]):
    print(f"{name} | {k} | {run / 1e9:.2f} | {wait / 1e9:.2f} | {wait / max(run, 1):.2f} | {slices} | {vol} | {invol}")
