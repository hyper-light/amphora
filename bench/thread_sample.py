#!/usr/bin/env python3
"""Lane V: where a replicated commit's time goes, for hosts without perf or strace.

Starts three `amphora-bench --repl-member` processes itself (Yama's ptrace_scope=1 lets only an
ancestor read /proc/<pid>/task/*/syscall), finds the leader, runs the commit driver with writer w
at member w % 3 ("spread", the ladder's routing) or every writer at the leader ("leader"), and from
--after seconds on samples every member thread's state, syscall and wchan, with each thread's CPU
and each member's I/O over the sampled span. Prints one JSON object a phase.

    bench/thread_sample.py BIN PORT DIR WRITERS ROUTE SECONDS AFTER SPAN [CLASS]

DIR is one folder (members in DIR/m1..m3) or three, comma-separated, one a member: a member on a
RAM disk beside two on disk tells whose flush a commit waits for.
"""
import collections
import json
import os
import socket
import subprocess
import sys
import time

NAMES = {0: "read", 1: "write", 3: "close", 7: "poll", 9: "mmap", 11: "munmap", 17: "pread64", 18: "pwrite64",
         19: "readv", 20: "writev", 24: "sched_yield", 28: "madvise", 35: "nanosleep", 43: "accept", 44: "sendto",
         45: "recvfrom", 46: "sendmsg", 47: "recvmsg", 74: "fsync", 75: "fdatasync", 77: "ftruncate",
         202: "futex", 230: "clock_nanosleep", 232: "epoll_wait", 257: "openat", 277: "sync_file_range",
         281: "epoll_pwait", 285: "fallocate", 288: "accept4", 295: "preadv", 296: "pwritev",
         426: "io_uring_enter", 441: "epoll_pwait2"}
HZ = os.sysconf("SC_CLK_TCK")


def rd(path):
    try:
        with open(path) as f:
            return f.read()
    except OSError:
        return None


def tids(pid):
    try:
        return os.listdir(f"/proc/{pid}/task")
    except OSError:
        return []


def cpu(pid, tid):
    s = rd(f"/proc/{pid}/task/{tid}/stat")
    if not s:
        return 0
    f = s[s.rfind(")") + 2:].split()
    return int(f[11]) + int(f[12])


def where(pid, tid):
    st = rd(f"/proc/{pid}/task/{tid}/stat")
    if not st:
        return None
    state = st[st.rfind(")") + 2:].split()[0]
    sc = (rd(f"/proc/{pid}/task/{tid}/syscall") or "unreadable").split()
    wchan = (rd(f"/proc/{pid}/task/{tid}/wchan") or "").strip()
    if not sc:
        name = "?"
    elif sc[0] in ("running", "unreadable"):
        name = sc[0]
    elif sc[0] == "-1":
        name = "user"
    else:
        nr = int(sc[0])
        name = NAMES.get(nr, str(nr))
        if nr in (0, 1, 17, 18, 19, 20, 74, 75, 295, 296) and len(sc) > 1:
            name += f" fd{int(sc[1], 16)}"
    return state, name, wchan


def ask(addr, obj):
    host, port = addr.rsplit(":", 1)
    with socket.create_connection((host, int(port)), timeout=5) as s:
        s.sendall(b"C" + (json.dumps(obj) + "\n").encode())
        buf = b""
        while not buf.endswith(b"\n"):
            d = s.recv(65536)
            if not d:
                break
            buf += d
    return json.loads(buf or b"{}")


def io(pid):
    out = {}
    for line in (rd(f"/proc/{pid}/io") or "").splitlines():
        k, _, v = line.partition(":")
        out[k] = int(v)
    return out


def main():
    binary, port, folder, writers, route, seconds, after, span = sys.argv[1:9]
    klass = sys.argv[9] if len(sys.argv) > 9 else "replicated"
    port, writers, seconds, after, span = int(port), int(writers), float(seconds), float(after), float(span)
    peers = [f"127.0.0.1:{port + i}" for i in range(3)]
    flags = ["--deadline-s", "10", "--relay-records", "128"] + (["--spanning", "2"] if klass == "replicated" else [])
    folders = folder.split(",")
    folders = folders if len(folders) == 3 else [f"{folder}/m{i + 1}" for i in range(3)]
    folder = folders[0]
    members = []
    for i in range(3):
        members.append(subprocess.Popen(
            [binary, "--repl-member", "--node", str(i + 1), "--peers", ",".join(peers), "--dir", folders[i],
             "--class", klass] + flags, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True))
    for m in members:
        m.stdout.readline()
    leader, t0 = 0, time.monotonic()
    while leader == 0 and time.monotonic() - t0 < 30:
        try:
            leader = int(ask(peers[0], {"op": "leader"}).get("leader") or 0)
        except (OSError, ValueError):
            pass
        if leader == 0:
            time.sleep(0.05)
    to = [peers[leader - 1]] * 3 if route == "leader" else peers
    record = f"{folder}/rec-{klass}-{writers}-{route}.jsonl"
    driver = subprocess.Popen(
        [binary, "--repl", "commit", "--system", "amphora-procs", "--class", klass, "--members", ",".join(to),
         "--record", record, "--batch", "1", "--text-bytes", "600", "--warmup-s", "1", "--seconds", str(seconds),
         "--writers", str(writers)] + flags, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    time.sleep(after)
    pids = [m.pid for m in members]
    c0 = {(p, t): cpu(p, t) for p in pids for t in tids(p)}
    io0 = {p: io(p) for p in pids}
    seen, sweeps, began = collections.Counter(), 0, time.monotonic()
    while time.monotonic() - began < span:
        for p in pids:
            for t in tids(p):
                w = where(p, t)
                if w:
                    seen[(p, t) + w] += 1
        sweeps += 1
    elapsed = time.monotonic() - began
    c1 = {(p, t): cpu(p, t) for p in pids for t in tids(p)}
    io1 = {p: io(p) for p in pids}
    comms = {(p, t): (rd(f"/proc/{p}/task/{t}/comm") or "?").strip() for p in pids for t in tids(p)}
    _, err = driver.communicate(timeout=300)
    for m in members:
        try:
            m.wait(timeout=15)
        except subprocess.TimeoutExpired:
            m.kill()
            m.wait()
    out = {"writers": writers, "route": route, "class": klass, "leader": leader, "sweeps": sweeps,
           "span_s": round(elapsed, 2), "driver_err_tail": err.strip().splitlines()[-3:]}
    try:
        r = json.loads(open(record).read().strip().splitlines()[-1])
        out["commits_s"] = r.get("writes_per_s")
        out["latency_ms"] = r.get("latency_ms")
        out["refused"] = r.get("refused")
    except (OSError, IndexError, ValueError) as e:
        out["record_error"] = str(e)
    per_member = []
    for i, p in enumerate(pids):
        threads = []
        for (pp, t), n in sorted(c1.items()):
            if pp != p:
                continue
            states = collections.Counter()
            for (sp, st, state, name, wchan), k in seen.items():
                if sp == p and st == t:
                    states[f"{state} {name} {wchan}"] += k
            top = [[s, round(k / max(sweeps, 1), 3)] for s, k in states.most_common(3)]
            threads.append({"tid": t, "comm": comms.get((p, t), "?"),
                            "cpu": round((n - c0.get((p, t), 0)) / HZ / elapsed, 3), "top": top})
        groups = collections.defaultdict(lambda: {"threads": 0, "cpu": 0.0})
        for th in threads:
            sig = (th["comm"], th["top"][0][0] if th["top"] else "-")
            groups[sig]["threads"] += 1
            groups[sig]["cpu"] = round(groups[sig]["cpu"] + th["cpu"], 3)
        busy = [th for th in threads if th["cpu"] >= 0.02 or th["comm"] == "hyper-log-io" or (th["top"] and not th["top"][0][0].startswith("S futex"))]
        d = {k: io1[p].get(k, 0) - io0[p].get(k, 0) for k in ("syscr", "syscw", "write_bytes", "wchar")}
        per_member.append({"node": i + 1, "is_leader": i + 1 == leader, "io_per_s": {k: round(v / elapsed) for k, v in d.items()},
                           "cpu_total": round(sum(th["cpu"] for th in threads), 3),
                           "groups": [{"comm": c, "top": s, **v} for (c, s), v in sorted(groups.items(), key=lambda kv: -kv[1]["cpu"])][:12],
                           "busy": busy[:16]})
    out["members"] = per_member
    print(json.dumps(out))


if __name__ == "__main__":
    main()
