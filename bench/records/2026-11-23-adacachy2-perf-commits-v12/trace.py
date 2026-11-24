#!/usr/bin/env python3
"""Lane V12: one traced commit run. Starts three members of BIN (built from diag/commits-v12-stages)
with AMPHORA_STAGES (the stage log) and lane W2's shim (LD_PRELOAD, STAGES_OUT: each log write and
flush syscall), runs the commit driver without either, and prints the driver's record.

    trace.py BIN SHIM_SO PORT OUT WRITERS ROUTE CLASS
"""
import json, os, socket, subprocess, sys, time

binary, shim, port, out, writers, route, klass = sys.argv[1:8]
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
    env = dict(os.environ, AMPHORA_STAGES=f"{out}/stages-m{i + 1}.log", STAGES_OUT=f"{out}/shim-m{i + 1}.rec", LD_PRELOAD=shim)
    members.append(subprocess.Popen([binary, "--repl-member", "--node", str(i + 1), "--peers", ",".join(peers), "--dir", d,
                                     "--class", klass] + flags, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, env=env))
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
print(json.dumps({"writers": writers, "route": route, "class": klass, "leader_at_start": leader, "driver_rc": driver.returncode,
                  "commits_s": r.get("writes_per_s"), "latency_ms": r.get("latency_ms"), "misses": r.get("deadline_misses"),
                  "err": driver.stderr.strip().splitlines()[-2:]}))
