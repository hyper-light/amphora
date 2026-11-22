#!/usr/bin/env python3
"""What a writer count of bench/commits.sh needs, learned from the rungs it already ran, against what
this host has (goal "Many clients write through SQL at once, safely"; T-T1.4.1, the brief's
"Concurrency ladder": bound and report connections and memory). A count the host can't hold is
refused before it runs, saying why, not run unfairly (one system swapping or out of descriptors
while the others aren't).

  commits_fit.py measure <learned.jsonl> <platform json> [--server-pid-file <file>] -- <driver...>
      runs the driver (`amphora-bench --powercut bench ... --hold`), and when it says `held` (its
      last commit in, every connection still open) reads what it holds: its open files, and for
      PostgreSQL the server's memory (the postmaster and its backends, proportional set size on
      Linux, so the shared buffers count once); then lets it end, and prints its record with the
      platform and the client's resources beside it, appending them to <learned.jsonl>.
  commits_fit.py check <learned.jsonl> <system> <writers> [--pg-shared-bytes <n>]
      prints one JSON line, {"fits": bool, "why": ..., "need": ..., "have": ...}; exit 0 when it
      fits, 2 when it doesn't. For PostgreSQL also the settings it sizes ("pg": shared_buffers and
      work_mem, see `pg_settings`).

A count's need is extrapolated from the two largest counts already run on this system (a line
through them: a fixed part and a part a writer), or from one (all of it a writer's: the most it can
be), so nothing is assumed about an engine before it is seen; the first count runs unchecked.
"""
import json
import os
import resource
import subprocess
import sys
import time

KEYS = ("driver_rss_bytes", "driver_fds", "server_bytes")


def read_learned(path, system):
    """This system's rungs so far: [(writers, {key: value})], by writers."""
    rungs = {}
    if os.path.exists(path):
        with open(path, encoding="utf-8") as f:
            for line in f:
                r = json.loads(line)
                if r.get("system") == system:
                    rungs[r["writers"]] = {k: r.get(k) for k in KEYS}
    return sorted(rungs.items())


def predict(rungs, key, writers):
    """`key`'s value at `writers`, from the rungs that measured it; None when none did."""
    seen = [(w, v[key]) for w, v in rungs if v.get(key) is not None]
    if not seen:
        return None
    if len(seen) == 1:
        (w, v), = seen
        return v * writers / w
    (w1, v1), (w2, v2) = seen[-2], seen[-1]
    slope = max(0.0, (v2 - v1) / (w2 - w1))
    return max(v2, v2 + slope * (writers - w2))


def meminfo():
    """(total, available) bytes this process may use: the machine's, bounded by its cgroup's."""
    if sys.platform.startswith("linux"):
        info = {}
        with open("/proc/meminfo", encoding="utf-8") as f:
            for line in f:
                k, v = line.split(":", 1)
                info[k] = int(v.split()[0]) * 1024
        total, avail = info["MemTotal"], info["MemAvailable"]
        try:
            with open("/proc/self/cgroup", encoding="utf-8") as f:
                rel = next(l.split(":", 2)[2].strip() for l in f if l.startswith("0::"))
            base = os.path.join("/sys/fs/cgroup", rel.lstrip("/"))
            for name in ("memory.max", "memory.high"):
                with open(os.path.join(base, name), encoding="utf-8") as f:
                    lim = f.read().strip()
                if lim != "max":
                    with open(os.path.join(base, "memory.current"), encoding="utf-8") as f:
                        cur = int(f.read())
                    total, avail = min(total, int(lim)), min(avail, int(lim) - cur)
        except (OSError, StopIteration, ValueError):
            pass
        return total, avail
    if sys.platform == "darwin":
        total = int(subprocess.run(["sysctl", "-n", "hw.memsize"], capture_output=True, text=True).stdout)
        out = subprocess.run(["vm_stat"], capture_output=True, text=True).stdout.splitlines()
        page = int(out[0].split("page size of")[1].split()[0])
        pages = {l.split(":")[0]: int(l.split(":")[1].strip().rstrip(".")) for l in out[1:] if ":" in l}
        free = sum(pages.get(k, 0) for k in ("Pages free", "Pages inactive", "Pages speculative", "Pages purgeable"))
        return total, free * page
    return None, None


def threads_room():
    """(threads this user may still start, why the bound): RLIMIT_NPROC less what runs on Linux,
    the per-process thread limit on macOS; None when there's no bound to check."""
    if sys.platform == "darwin":
        n = int(subprocess.run(["sysctl", "-n", "kern.num_taskthreads"], capture_output=True, text=True).stdout)
        return n, "kern.num_taskthreads, a process's threads"
    if sys.platform.startswith("linux"):
        soft, _ = resource.getrlimit(resource.RLIMIT_NPROC)
        if soft == resource.RLIM_INFINITY:
            return None, None
        uid, used = os.getuid(), 0
        for pid in os.listdir("/proc"):
            if pid.isdigit():
                try:
                    if os.stat(f"/proc/{pid}").st_uid == uid:
                        used += len(os.listdir(f"/proc/{pid}/task"))
                except OSError:
                    pass
        return soft - used, f"RLIMIT_NPROC {soft} less this user's {used} threads"
    return None, None


def open_files_room():
    """Open files a new process may hold: the soft limit (commits.sh raised it to the hard one)."""
    soft, _ = resource.getrlimit(resource.RLIMIT_NOFILE)
    return None if soft == resource.RLIM_INFINITY else soft


def pg_settings(total, room, writers, shared_bytes, backends):
    """PostgreSQL's shared_buffers and work_mem for `writers` connections in `room` bytes, or None.
    shared_buffers: a quarter of the memory, PostgreSQL's own advice ("a reasonable starting value
    for shared_buffers is 25% of the memory in your system", runtime-config-resource, 20.4.1; it
    leaves the rest to the operating system's cache, which PostgreSQL relies on), of the machine or
    of what is left beside the backends and their shared memory, whichever is less. work_mem: its
    default, 4MB, for each writer's sort or hash (the bench's single-row INSERT does neither, so
    none is used; it is sized so even one each fits), less what doesn't fit, down to its minimum,
    64kB; shared_buffers' minimum is 128kB (16 buffers). Both in kB, PostgreSQL's unit."""
    kb = 1024
    rest = int(room - backends - shared_bytes)
    sb = min(int(total) // 4, rest // 4)
    if sb < 128 * kb:
        return None
    wm = min(4096 * kb, (rest - sb) // writers)
    if wm < 64 * kb:
        return None
    return {"shared_buffers_kb": sb // kb, "work_mem_kb": wm // kb}


def check(learned, system, writers, pg_shared=None):
    rungs = read_learned(learned, system)
    total, avail = meminfo()
    need = {k: predict(rungs, k, writers) for k in KEYS}
    out = {"system": system, "writers": writers, "need": need, "have": {"memory_total": total, "memory_available": avail},
           "from_rungs": [w for w, _ in rungs]}
    why = []
    threads, bound = threads_room()
    driver_threads = writers + 1
    server_procs = writers if system == "postgres" else 0
    if threads is not None:
        out["have"]["threads"] = threads
        if driver_threads + (server_procs if sys.platform.startswith("linux") else 0) > threads:
            why.append(f"{driver_threads} driver threads{' and ' + str(server_procs) + ' backends' if server_procs else ''}: {bound} leaves {threads}")
    files = open_files_room()
    out["have"]["open_files"] = files
    if files is not None and need["driver_fds"] is not None and need["driver_fds"] > files:
        why.append(f"the driver would hold {need['driver_fds']:.0f} open files (from counts {out['from_rungs']}); the limit is {files}")
    mem = sum(v for v in (need["driver_rss_bytes"], need["server_bytes"]) if v is not None)
    if system == "postgres" and avail is not None:
        room = avail - (need["driver_rss_bytes"] or 0)
        pg = pg_settings(total, room, writers, pg_shared or 0, need["server_bytes"] or 0)
        if pg is None:
            why.append(f"PostgreSQL's {writers} backends ({(need['server_bytes'] or 0) / 2**20:.0f} MiB) and shared memory"
                       f" ({(pg_shared or 0) / 2**20:.0f} MiB) leave no room for its minimum shared_buffers and work_mem in"
                       f" {room / 2**20:.0f} MiB available beside the driver")
        out["pg"] = pg
    if avail is not None and mem > avail:
        why.append(f"it would take {mem / 2**20:.0f} MiB (from counts {out['from_rungs']}); {avail / 2**20:.0f} MiB is available")
    out["fits"] = not why
    out["why"] = "; ".join(why) if why else None
    print(json.dumps(out), flush=True)
    return 0 if out["fits"] else 2


def driver_fds(pid):
    """The driver's open files: /proc on Linux; on macOS libproc's PROC_PIDLISTFDS (1), whose
    answer is an array of struct proc_fdinfo (two int32s: 8 bytes each), <sys/proc_info.h>."""
    if os.path.isdir(f"/proc/{pid}/fd"):
        return len(os.listdir(f"/proc/{pid}/fd"))
    if sys.platform == "darwin":
        import ctypes
        libproc = ctypes.CDLL("/usr/lib/libproc.dylib")
        size = libproc.proc_pidinfo(pid, 1, 0, None, 0)
        if size <= 0:
            return None
        buf = ctypes.create_string_buffer(size)
        got = libproc.proc_pidinfo(pid, 1, 0, buf, size)
        return got // 8 if got > 0 else None
    return None


def server_bytes(postmaster):
    """The server's memory: the postmaster and its children; on Linux their proportional set sizes
    (each shared page divided among the processes sharing it), elsewhere their resident sizes
    (shared pages counted in each: an overcount)."""
    if os.path.isdir("/proc"):
        pids, total = [postmaster], 0
        for pid in os.listdir("/proc"):
            if pid.isdigit():
                try:
                    with open(f"/proc/{pid}/stat", encoding="utf-8") as f:
                        if int(f.read().rsplit(")", 1)[1].split()[1]) == postmaster:
                            pids.append(int(pid))
                except (OSError, ValueError, IndexError):
                    pass
        for pid in pids:
            try:
                with open(f"/proc/{pid}/smaps_rollup", encoding="utf-8") as f:
                    total += next(int(l.split()[1]) for l in f if l.startswith("Pss:")) * 1024
            except (OSError, StopIteration):
                pass
        return total, len(pids), "pss"
    got = subprocess.run(["ps", "-axo", "pid=,ppid=,rss="], capture_output=True, text=True).stdout.split("\n")
    rows = [tuple(int(x) for x in l.split()) for l in got if l.strip()]
    mine = [r for r in rows if r[0] == postmaster or r[1] == postmaster]
    return sum(r[2] for r in mine) * 1024, len(mine), "rss"


def measure(learned, platform, server_pid_file, cmd, system):
    began = time.monotonic()
    p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    held = {}
    for line in p.stderr:
        if line.strip() == b"held":
            held["driver_fds"] = driver_fds(p.pid)
            if server_pid_file:
                with open(server_pid_file, encoding="utf-8") as f:
                    pm = int(f.readline())
                held["server_bytes"], held["server_processes"], held["server_measure"] = server_bytes(pm)
            p.stdin.close()
        else:
            sys.stderr.buffer.write(line)
    stdout = p.stdout.read()
    # The driver's own resources (wait4), not those of the probes run beside it (lsof, ps).
    _, status, use = os.wait4(p.pid, 0)
    p.returncode = rc = os.waitstatus_to_exitcode(status)
    wall = time.monotonic() - began
    if rc != 0:
        sys.exit(f"{' '.join(cmd)}: exit {rc}")
    r = json.loads(stdout.decode())
    rss = use.ru_maxrss * (1 if sys.platform == "darwin" else 1024)
    cpu = use.ru_utime + use.ru_stime
    r["platform"] = platform
    r["client"] = {
        "driver": r.get("client"),
        "cpu_user_s": use.ru_utime,
        "cpu_sys_s": use.ru_stime,
        "process_s": wall,
        "cores_busy": cpu / wall if wall > 0 else None,
        "cores": os.cpu_count(),
        "max_rss_bytes": rss,
        "connections": r.get("writers"),
        "held": held,
    }
    with open(learned, "a", encoding="utf-8") as f:
        f.write(json.dumps({"system": system, "writers": r.get("writers"), "driver_rss_bytes": rss,
                            "driver_fds": held.get("driver_fds"), "server_bytes": held.get("server_bytes")}) + "\n")
    print(json.dumps(r), flush=True)
    return 0


def main(argv):
    if len(argv) >= 4 and argv[0] == "check":
        pg_shared = int(argv[argv.index("--pg-shared-bytes") + 1]) if "--pg-shared-bytes" in argv else None
        return check(argv[1], argv[2], int(argv[3]), pg_shared)
    if len(argv) >= 5 and argv[0] == "measure" and "--" in argv:
        cut = argv.index("--")
        opts = argv[1:cut]
        spf = opts[opts.index("--server-pid-file") + 1] if "--server-pid-file" in opts else None
        system = opts[opts.index("--system") + 1] if "--system" in opts else None
        return measure(opts[0], json.loads(opts[1]), spf, argv[cut + 1:], system)
    sys.exit(__doc__)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
