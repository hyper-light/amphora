#!/usr/bin/env python3
"""Where a bench run's threads wait, for hosts without perf or strace (AUDIT 2, the 32-client ceiling).

Starts the command itself so it is the command's ancestor (Yama's ptrace_scope=1 lets only an
ancestor read /proc/<pid>/task/*/syscall), then, from --after seconds on, samples every thread's
state and the syscall it sits in, and its CPU and context switches over the sampled span.

    bench/syscall_sample.py --after 3 --span 5 -- amphora-bench --target amphora ... --clients 32

Prints one JSON object: samples by (state, syscall, first argument) with each one's share, the
sampled span's user and system CPU seconds, and voluntary and involuntary switches a second.
"""
import argparse, collections, json, os, subprocess, sys, time

NAMES = {0: "read", 1: "write", 7: "poll", 17: "pread64", 18: "pwrite64", 19: "readv", 20: "writev",
         24: "sched_yield", 35: "nanosleep", 74: "fsync", 75: "fdatasync", 202: "futex", 230: "clock_nanosleep",
         232: "epoll_wait", 281: "epoll_pwait", 295: "preadv", 296: "pwritev", 426: "io_uring_enter", 441: "epoll_pwait2"}
FUTEX_OPS = {0: "WAIT", 1: "WAKE", 9: "WAIT_BITSET", 10: "WAKE_BITSET", 128: "WAIT_PRIVATE", 129: "WAKE_PRIVATE",
             137: "WAIT_BITSET_PRIVATE", 138: "WAKE_BITSET_PRIVATE"}
TICK = os.sysconf("SC_CLK_TCK")


def read(path):
    try:
        with open(path) as f:
            return f.read()
    except OSError:
        return None


def tasks(pid):
    try:
        return os.listdir(f"/proc/{pid}/task")
    except OSError:
        return []


def totals(pid):
    user = system = vol = invol = 0
    for tid in tasks(pid):
        stat = read(f"/proc/{pid}/task/{tid}/stat")
        status = read(f"/proc/{pid}/task/{tid}/status")
        if stat:
            fields = stat.rsplit(")", 1)[1].split()
            user += int(fields[11])
            system += int(fields[12])
        for line in (status or "").splitlines():
            if line.startswith("voluntary_ctxt_switches"):
                vol += int(line.split()[1])
            elif line.startswith("nonvoluntary_ctxt_switches"):
                invol += int(line.split()[1])
    return user / TICK, system / TICK, vol, invol


def where(pid, tid):
    stat = read(f"/proc/{pid}/task/{tid}/stat")
    if not stat:
        return None
    state = stat.rsplit(")", 1)[1].split()[0]
    syscall = (read(f"/proc/{pid}/task/{tid}/syscall") or "unreadable").split()
    if not syscall or syscall[0] in ("running", "unreadable"):
        return (state, syscall[0] if syscall else "?", "")
    if syscall[0] == "-1":
        return (state, "not-in-syscall", "")
    nr = int(syscall[0])
    name = NAMES.get(nr, str(nr))
    arg = ""
    if nr == 202 and len(syscall) > 2:
        arg = FUTEX_OPS.get(int(syscall[2], 16), syscall[2]) + " @" + syscall[1]
    elif nr in (0, 1, 17, 18, 74, 75) and len(syscall) > 1:
        arg = "fd " + str(int(syscall[1], 16))
    return (state, name, arg)


def region(pid, address):
    """The mapping holding `address`, and for the executable's own, the symbol at or below it."""
    for line in (read(f"/proc/{pid}/maps") or "").splitlines():
        fields = line.split()
        start, end = (int(x, 16) for x in fields[0].split("-"))
        if start <= address < end:
            path = fields[5] if len(fields) > 5 else "anon"
            if path.startswith("/") and os.access(path, os.X_OK):
                return path, int(fields[2], 16) + address - start
            return path, None
    return "unmapped", None


def symbol(binary, address):
    out = subprocess.run(["nm", "-C", "-n", "--defined-only", binary], capture_output=True, text=True).stdout
    best = None
    for line in out.splitlines():
        parts = line.split(" ", 2)
        if len(parts) == 3:
            try:
                at = int(parts[0], 16)
            except ValueError:
                continue
            if at > address:
                break
            best = (hex(address - at), parts[2])
    return best


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--after", type=float, required=True, help="seconds from start to the first sample")
    parser.add_argument("--span", type=float, required=True, help="seconds to sample for")
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    child = subprocess.Popen(command, stdout=subprocess.PIPE, text=True)
    time.sleep(args.after)
    began = time.monotonic()
    user0, sys0, vol0, invol0 = totals(child.pid)
    seen = collections.Counter()
    samples = 0
    while time.monotonic() - began < args.span and child.poll() is None:
        for tid in tasks(child.pid):
            at = where(child.pid, tid)
            if at:
                seen[at] += 1
        samples += 1
    user1, sys1, vol1, invol1 = totals(child.pid)
    futexes = collections.Counter()
    for (state, name, arg), n in seen.items():
        if name == "futex" and "@" in arg:
            futexes[arg.split("@")[1]] += n
    places = []
    for address, n in futexes.most_common(6):
        path, offset = region(child.pid, int(address, 16))
        places.append({"futex": address, "samples": n, "mapping": path,
                       "symbol": symbol(path, offset) if offset is not None else None})
    span = time.monotonic() - began
    out, _ = child.communicate()
    total = sum(seen.values()) or 1
    print(json.dumps({
        "command": command, "sweeps": samples, "span_s": round(span, 2),
        "cpu_user_s": round(user1 - user0, 2), "cpu_sys_s": round(sys1 - sys0, 2),
        "voluntary_per_s": round((vol1 - vol0) / span), "involuntary_per_s": round((invol1 - invol0) / span),
        "futexes": places,
        "where": [{"state": s, "syscall": c, "arg": a, "share": round(n / total, 4)} for (s, c, a), n in seen.most_common(15)],
        "bench_last_line": (out.strip().splitlines() or [""])[-1][:2000],
    }))
    return child.returncode


if __name__ == "__main__":
    sys.exit(main())
