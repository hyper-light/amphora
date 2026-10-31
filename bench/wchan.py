# Samples where named threads of a process wait in the kernel (/proc/<pid>/task/<tid>/wchan and
# state, readable without root) every millisecond, and reports each wait that lasted 20 ms or more,
# and the totals. usage: python3 wchan.py <process name> <seconds> [thread name prefix ...]
import os, sys, time, collections

name, seconds = sys.argv[1], float(sys.argv[2])
prefixes = sys.argv[3:] or ["amphora-"]
deadline = time.time() + 60
pid = None
while pid is None and time.time() < deadline:
    for p in os.listdir("/proc"):
        if p.isdigit():
            try:
                if open(f"/proc/{p}/comm").read().strip() == name:
                    pid = p
            except OSError:
                pass
    time.sleep(0.05)
if pid is None:
    sys.exit("no such process")

def threads():
    out = {}
    for t in os.listdir(f"/proc/{pid}/task"):
        try:
            comm = open(f"/proc/{pid}/task/{t}/comm").read().strip()
        except OSError:
            continue
        if any(comm.startswith(x) for x in prefixes):
            out[t] = comm
    return out

time.sleep(1)
ts = threads()
totals = collections.Counter()
runs = {t: (None, 0.0) for t in ts}
long_waits = []
end = time.time() + seconds
while time.time() < end:
    now = time.time()
    for t, comm in ts.items():
        try:
            state = open(f"/proc/{pid}/task/{t}/stat").read().rsplit(")", 1)[1].split()[0]
            wchan = open(f"/proc/{pid}/task/{t}/wchan").read().strip() or "-"
        except OSError:
            continue
        key = (state, wchan) if state in "DS" else ("R", "-")
        totals[(comm, key)] += 1
        prev, since = runs[t]
        if key != prev:
            if prev is not None and prev[0] != "R" and now - since >= 0.020:
                long_waits.append((comm, prev, round((now - since) * 1000)))
            runs[t] = (key, now)
    time.sleep(0.001)
print("== waits of 20 ms or more ==")
for w in sorted(long_waits, key=lambda w: -w[2])[:40]:
    print(w)
print("== samples ==")
for (comm, key), n in totals.most_common(40):
    print(comm, key, n)
