#!/usr/bin/env python3
"""Lane P23: V13's commit cell (cell13.py), with each member's threads' CPU and run-queue wait read from
/proc/<pid>/task/*/schedstat while the driver runs (every half second, by this script, outside the members).

    cell23.py BIN PORT OUT WRITERS ROUTE CLASS
"""
import json, os, socket, subprocess, sys, time, threading, collections

binary, port, out, writers, route, klass = sys.argv[1:7]
port = int(port)
os.makedirs(out, exist_ok=True)
peers = [f"127.0.0.1:{port + i}" for i in range(3)]
flags = ["--deadline-s", "10", "--relay-records", "128"] + (["--spanning", "2"] if klass == "replicated" else [])


def ask(addr, obj):
    host, p = addr.rsplit(":", 1)
    with socket.create_connection((host, int(p)), timeout=5) as s:
        s.sendall(b"C" + (json.dumps(obj) + "\n").encode())
        buf = b""
        while not buf.endswith(b"\n"):
            d = s.recv(65536)
            if not d:
                break
            buf += d
    return json.loads(buf or b"{}")


def threads(pid):
    got = {}
    try:
        tids = os.listdir(f"/proc/{pid}/task")
    except OSError:
        return got
    for t in tids:
        try:
            comm = open(f"/proc/{pid}/task/{t}/comm").read().strip()
            run, wait, slices = open(f"/proc/{pid}/task/{t}/schedstat").read().split()
            got[t] = (comm, int(run), int(wait), int(slices))
        except (OSError, ValueError):
            pass
    return got


members = []
for i in range(3):
    d = f"{out}/m{i + 1}"
    subprocess.run(["rm", "-rf", d])
    members.append(subprocess.Popen([binary, "--repl-member", "--node", str(i + 1), "--peers", ",".join(peers), "--dir", d,
                                     "--class", klass] + flags, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True))
for m in members:
    m.stdout.readline()
leader, t0 = 0, time.monotonic()
while leader == 0 and time.monotonic() - t0 < 30:
    try:
        leader = int(ask(peers[0], {"op": "leader"}).get("leader") or 0)
    except (OSError, ValueError):
        pass
samples, stop = [], threading.Event()


def watch():
    while not stop.wait(0.5):
        samples.append((time.monotonic(), [threads(m.pid) for m in members]))


w = threading.Thread(target=watch)
w.start()
record = f"{out}/rec.jsonl"
driver = subprocess.run([binary, "--repl", "commit", "--system", "amphora-procs", "--class", klass, "--members", ",".join(peers),
                         "--record", record, "--batch", "1", "--text-bytes", "600", "--warmup-s", "1", "--seconds", "5",
                         "--writers", writers, "--route", route] + flags, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
stop.set()
w.join()
for m in members:
    try:
        m.wait(timeout=30)
    except subprocess.TimeoutExpired:
        m.terminate()
        m.wait()
r = json.loads(open(record).read().strip().splitlines()[-1]) if os.path.exists(record) else {}
lat = r.get("latency_ms") or {}
# The middle of the run: from the sample nearest 2 s in to the one nearest 5 s in.
cpu = {}
if len(samples) > 8:
    a, b = samples[3], samples[min(9, len(samples) - 1)]
    span = b[0] - a[0]
    for i in range(3):
        by = collections.defaultdict(lambda: [0, 0, 0, 0])
        for t, (comm, run, wait, slices) in b[1][i].items():
            z = a[1][i].get(t, (comm, 0, 0, 0))
            e = by[comm]
            e[0] += run - z[1]
            e[1] += wait - z[2]
            e[2] += slices - z[3]
            e[3] += 1
        cpu[f"m{i + 1}{'L' if i + 1 == leader else ''}"] = {
            c: f"n{e[3]} cpu{e[0] / 1e9 / span:.2f} wait{e[1] / 1e9 / span:.2f} sl/s{e[2] / span:.0f}" for c, e in sorted(by.items(), key=lambda kv: -kv[1][0]) if e[0] > 1e6 or e[2] > 20}
print(json.dumps({"class": klass, "writers": writers, "route": route, "leader": leader, "rc": driver.returncode,
                  "commits_s": r.get("writes_per_s"), "p50": lat.get("p50"), "p99": lat.get("p99"),
                  "refused_by_node": [n.get("refused") for n in r.get("by_node", [])], "misses": r.get("deadline_misses"),
                  "threads": cpu,
                  "err": driver.stderr.strip().splitlines()[-2:]}))
