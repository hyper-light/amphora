#!/usr/bin/env python3
"""Lane Q1: where each thread of a suite run spends its time, per phase (the suite's "== <target>
<op> clients=N" lines): every thread's state and syscall (and wchan when asleep) sampled as fast as
/proc allows, each thread's run and run-queue wait (schedstat), and the process's read syscalls
and bytes (/proc/<pid>/io). Starts the bench itself: Yama's ptrace_scope=1 lets only an ancestor
read /proc/<pid>/task/*/syscall and io. usage: q1_sample.py OUT.json BIN ARGS..."""
import collections, json, os, subprocess, sys, threading, time

NAMES = {0: "read", 1: "write", 7: "poll", 9: "mmap", 11: "munmap", 17: "pread64", 18: "pwrite64", 19: "readv",
         24: "sched_yield", 28: "madvise", 35: "nanosleep", 74: "fsync", 75: "fdatasync", 202: "futex",
         230: "clock_nanosleep", 232: "epoll_wait", 257: "openat", 295: "preadv", 296: "pwritev"}
out, cmd = sys.argv[1], sys.argv[2:]
p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
phase = ["setup"]
stdout = []


def err():
    for line in p.stderr:
        if line.startswith("== "):
            phase[0] = line.strip()[3:]
        sys.stderr.write(line)


threading.Thread(target=err, daemon=True).start()
threading.Thread(target=lambda: stdout.extend(p.stdout), daemon=True).start()


def rd(path):
    try:
        with open(path) as f:
            return f.read()
    except OSError:
        return None


def io():
    t = rd(f"/proc/{p.pid}/io") or ""
    return {k: int(v) for k, v in (l.split(": ") for l in t.splitlines() if ": " in l)}


def sched():
    run = wait = 0
    try:
        tasks = os.listdir(f"/proc/{p.pid}/task")
    except OSError:
        return None
    for t in tasks:
        s = (rd(f"/proc/{p.pid}/task/{t}/schedstat") or "0 0 0").split()
        run += int(s[0])
        wait += int(s[1])
    return {"run_s": run / 1e9, "runq_wait_s": wait / 1e9}


tally = collections.defaultdict(collections.Counter)
marks = {}
last = None
while p.poll() is None:
    ph = phase[0]
    if ph != last:
        now = {"t": time.time(), "io": io(), "sched": sched()}
        if last is not None:
            marks[last]["end"] = now
        marks[ph] = {"start": now}
        last = ph
    try:
        tasks = os.listdir(f"/proc/{p.pid}/task")
    except OSError:
        break
    for t in tasks:
        base = f"/proc/{p.pid}/task/{t}"
        st = rd(base + "/stat")
        if not st:
            continue
        comm = st[st.find("(") + 1:st.rfind(")")].rstrip("0123456789-")
        state = st[st.rfind(")") + 2]
        what = state
        if state != "R":
            sc = (rd(base + "/syscall") or "").split()
            if sc and sc[0].lstrip("-").isdigit():
                what += ":" + NAMES.get(int(sc[0]), sc[0])
            w = (rd(base + "/wchan") or "").strip()
            if w and w != "0":
                what += "@" + w
        tally[ph][comm + "|" + what] += 1
    time.sleep(0.001)
if last is not None and "end" not in marks[last]:
    marks[last]["end"] = {"t": time.time(), "io": {}, "sched": None}
res = {}
for ph, m in marks.items():
    s, e = m["start"], m.get("end", {})
    d = {"seconds": round(e.get("t", s["t"]) - s["t"], 3)}
    for k in ("syscr", "rchar", "read_bytes"):
        if k in s["io"] and k in e.get("io", {}):
            d[k] = e["io"][k] - s["io"][k]
    if s["sched"] and e.get("sched"):
        d["run_s"] = round(e["sched"]["run_s"] - s["sched"]["run_s"], 3)
        d["runq_wait_s"] = round(e["sched"]["runq_wait_s"] - s["sched"]["runq_wait_s"], 3)
    total = sum(tally[ph].values())
    d["samples"] = total
    d["states"] = {k: round(v * 100 / total, 1) for k, v in tally[ph].most_common(14)} if total else {}
    res[ph] = d
json.dump({"phases": res, "rc": p.returncode}, open(out, "w"), indent=1)
for ph, d in res.items():
    print(ph, json.dumps(d))
