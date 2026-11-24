#!/usr/bin/env python3
"""Lane A26: V13 cell13.py, printing also the diag driver's failures by kind. One Amphora commit cell. Starts three members of BIN, runs the commit driver with V12's
ladder flags, and prints its record's rate, latencies, refusals by node and deadline misses.

    cell13.py BIN PORT OUT WRITERS ROUTE CLASS
"""
import json, os, socket, subprocess, sys, time

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
record = f"{out}/rec.jsonl"
driver = subprocess.run([binary, "--repl", "commit", "--system", "amphora-procs", "--class", klass, "--members", ",".join(peers),
                         "--record", record, "--batch", "1", "--text-bytes", "600", "--warmup-s", "1", "--seconds", "5",
                         "--writers", writers, "--route", route] + flags, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
for m in members:
    try:
        m.wait(timeout=30)
    except subprocess.TimeoutExpired:
        m.terminate()
        m.wait()
r = json.loads(open(record).read().strip().splitlines()[-1]) if os.path.exists(record) else {}
lat = r.get("latency_ms") or {}
print(json.dumps({"class": klass, "writers": writers, "route": route, "leader": leader, "rc": driver.returncode,
                  "commits_s": r.get("writes_per_s"), "p50": lat.get("p50"), "p99": lat.get("p99"),
                  "refused_by_node": [n.get("refused") for n in r.get("by_node", [])], "misses": r.get("deadline_misses"),
                  "kinds": [l for l in driver.stderr.splitlines() if l.startswith("A26")], "err": [l for l in driver.stderr.strip().splitlines() if not l.startswith("A26")][-2:]}))
