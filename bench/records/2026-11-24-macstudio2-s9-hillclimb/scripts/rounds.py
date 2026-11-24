#!/usr/bin/env python3
"""Lane S9: the 17 statements of lane S7's suite at 1, 32 and 128 clients, five Amphora/competitor arms and two
durability-matched controls, in rounds. One client (amphora-bench --suite), one process a pass, the arms' order
rotated every round, host state recorded at every pass and round.
usage: rounds.py OUT_DIR FIRST_ROUND LAST_ROUND ORDERS   (ORDERS: one round's arms comma-separated, rounds
separated by '/', round N takes the Nth)
Records: OUT_DIR/round<N>.jsonl, one line a pass, as bench/run.sh writes them (the report plus host facts), the arm's
name in "arm" and the report's own target left as the engine's. Host samples: OUT_DIR/phases.log (every 5 s)."""
import datetime, json, os, re, resource, signal, subprocess, sys, threading, time
S9 = os.path.expanduser("~/.cache/amphora-s9")
ONLY = "point_pk$,point_secondary$,point_text$,json_field$,blob_read$,insert_one$,insert_batch_100$,update_plain$,update_indexed$,upsert$,delete_insert$,txn_read_write$,table_count$,table_group_by$,filter_no_index$,aggregates_range$,join_three$"
WRITES = "insert_one$,insert_batch_100$,update_plain$,update_indexed$,upsert$,delete_insert$,txn_read_write$"
OLD = {"AMPHORA_SHARED_PAGES": "0", "AMPHORA_READ_CHECKOUT": "pool", "AMPHORA_PAGE_PATH": "today", "AMPHORA_WRITER_PAGES": "1024"}
FULL = {"DYLD_INSERT_LIBRARIES": S9 + "/bin/libs9sync.dylib", "S9_FULLFSYNC": "1"}
ARMS = {  # name: (binary, engine target, environment, statements)
    "A": ("a", "amphora", {}, ONLY),
    "B": ("b", "amphora", {}, ONLY),
    "Bd": ("b", "amphora", OLD, ONLY),
    "C": ("c", "amphora", {}, ONLY),
    "S": ("a", "sqlite", {}, ONLY),
    "T": ("a", "turso", {}, ONLY),
    "Sf": ("a", "sqlite", FULL, WRITES),
    "Tf": ("a", "turso", FULL, WRITES),
}


def sh(cmd, t=20):
    try:
        return subprocess.run(cmd, shell=True, capture_output=True, text=True, timeout=t).stdout.strip()
    except Exception as e:
        return f"error {e}"


def now():
    return datetime.datetime.utcnow().strftime("%Y-%m-%dT%H:%M:%SZ")


def idle():
    """CPU user, sys and idle % over one second: top's second sample (its first is since boot)."""
    m = re.findall(r"CPU usage: ([\d.]+)% user, ([\d.]+)% sys, ([\d.]+)% idle", sh("top -l 2 -n 0 -s 1", 15))
    return [float(x) for x in m[-1]] if m else None


def mem():
    m = re.search(r"free percentage: (\d+)%", sh("memory_pressure -Q"))
    return {"free_pct": int(m.group(1)) if m else None, "vm_pressure_level": sh("sysctl -n kern.memorystatus_vm_pressure_level"),
            "swap": sh("sysctl -n vm.swapusage")}


def others(mine):
    """CPU (percent of a core) of every process that isn't ours, the five biggest."""
    out = []
    for line in sh("ps -Ao pcpu=,pid=,ppid=,comm= -r").splitlines()[:40]:
        p = line.split(None, 3)
        if len(p) == 4 and int(p[1]) not in mine and int(p[2]) not in mine:
            out.append((float(p[0]), p[3].rsplit("/", 1)[-1]))
    return {"sum": round(sum(x for x, _ in out), 1), "top": out[:5]}


def load():
    return [float(x) for x in sh("sysctl -n vm.loadavg").strip("{} ").split()[:3]]


MINE = {os.getpid()}


def sampler(path, stop):
    while not stop.wait(5):
        with open(path, "a") as f:
            f.write(json.dumps({"t": now(), "load": load(), "others": others(MINE)}) + "\n")


def state(label):
    return {"label": label, "t": now(), "load": load(), "cpu_user_sys_idle": idle(), "mem": mem(),
            "therm": sh("pmset -g therm").replace("\n", " | "), "others": others(MINE)}


def run_pass(arm, rnd, pos, out):
    binary, target, env, only = ARMS[arm]
    path = f"{S9}/bin/amphora-bench-{binary}"
    data = f"{S9}/data/{arm}"
    os.makedirs(data, exist_ok=True)
    e = dict(os.environ)
    for k in [k for k in e if k.startswith(("AMPHORA_", "DYLD_", "S9_", "KILN_"))]:
        e.pop(k)
    e.update(env)
    e["XDG_CACHE_HOME"] = S9 + "/xdg"
    secs, warm, clients = os.environ.get("S9_SECONDS", "1"), os.environ.get("S9_WARMUP", "0.5"), os.environ.get("S9_CLIENTS", "1,32,128")
    cmd = [path, "--target", target, "--dir", data, "--suite", "--only", only, "--items", "200000",
           "--clients", clients, "--seconds", secs, "--warmup", warm]
    before = {"load": load(), "others": others(MINE)}
    t0 = time.time()
    p = subprocess.Popen(cmd, env=e, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    MINE.add(p.pid)
    got = {}
    readers = [threading.Thread(target=lambda: got.__setitem__("o", p.stdout.read())), threading.Thread(target=lambda: got.__setitem__("e", p.stderr.read()))]
    for t in readers:
        t.start()
    late = threading.Timer(420, p.terminate)
    late.start()
    _, status, ru = os.wait4(p.pid, 0)
    late.cancel()
    p.returncode = os.waitstatus_to_exitcode(status)
    for t in readers:
        t.join()
    o, err = got.get("o", ""), got.get("e", "")
    took = time.time() - t0
    MINE.discard(p.pid)
    rec = {"arm": arm, "round": rnd, "position": pos, "date": now(), "took_s": round(took, 1), "rc": p.returncode, "env": env, "args": cmd[1:],
           "max_rss_mb": round(ru.ru_maxrss / 2**20, 1), "cpu_user_s": round(ru.ru_utime, 1), "cpu_sys_s": round(ru.ru_stime, 1), "ctx_vol": ru.ru_nvcsw, "ctx_invol": ru.ru_nivcsw, "majflt": ru.ru_majflt,
           "load_before": before["load"], "others_before": before["others"], "load_after": load(), "others_after": others(MINE)}
    try:
        rec["report"] = json.loads(o.strip().splitlines()[-1])
    except Exception as x:
        rec["failed"] = True
        rec["error"] = f"{x}: {err[-1500:]}"
    rec["stderr_tail"] = "\n".join(l for l in err.splitlines() if not l.startswith("== "))[-800:]
    with open(f"{out}/round{rnd}.jsonl", "a") as f:
        f.write(json.dumps(rec) + "\n")
    r = rec.get("report") or {}
    errs = sum((x.get("errors") or 0) for x in r.get("results") or [])
    print(f"{now()} round {rnd} pos {pos} {arm:3} {took:6.1f}s rc={p.returncode} cells={len(r.get('results') or [])} errors={errs} "
          f"load {before['load'][0]}->{rec['load_after'][0]} others {before['others']['sum']}% maxrss {rec['max_rss_mb']} heap_peak {r.get('heap_peak_mb')} {rec['stderr_tail'][-120:]!r}", flush=True)


def main():
    out, first, last = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
    orders = [o.split(",") for o in sys.argv[4].split("/")]
    os.makedirs(out, exist_ok=True)
    soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
    for want in (61440, 24576, 10240):
        try:
            resource.setrlimit(resource.RLIMIT_NOFILE, (want, hard))
            break
        except (ValueError, OSError):
            continue
    try:
        os.setpriority(os.PRIO_PROCESS, 0, 10)
    except OSError as x:
        print("nice", x, flush=True)
    print("nice", os.getpriority(os.PRIO_PROCESS, 0), "nofile", resource.getrlimit(resource.RLIMIT_NOFILE), flush=True)
    stop = threading.Event()
    threading.Thread(target=sampler, args=(f"{out}/phases.log", stop), daemon=True).start()
    for rnd in range(first, last + 1):
        order = orders[(rnd - 1) % len(orders)]
        if os.path.exists(f"{out}/order{rnd}.txt"):
            order = open(f"{out}/order{rnd}.txt").read().strip().split(",")
        with open(f"{out}/rounds.log", "a") as f:
            f.write(json.dumps({"round": rnd, "order": order, **state("start")}) + "\n")
        for pos, arm in enumerate(order, 1):
            run_pass(arm, rnd, pos, out)
        with open(f"{out}/rounds.log", "a") as f:
            f.write(json.dumps({"round": rnd, "order": order, **state("end")}) + "\n")
    stop.set()


main()
