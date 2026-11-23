#!/usr/bin/env python3
"""Lane V8: change 0009's halves against each other, from one binary whose halves the environment picks
(AB_READS_OFF, AB_BEATS_OFF; branch perf/commits-v8-ab). Starts three members, runs the commit driver with
writer w at member w % 3, and with READS_SPAN > 0 times sequential point reads (`Database::query`, the read
barrier's path) at the leader and at one follower over READS_SPAN seconds from AFTER on. Prints one JSON object.

    bench/v8_ab.py BIN PORT DIR WRITERS SECONDS AFTER READS_SPAN [CLASS]
"""
import json
import random
import socket
import subprocess
import sys
import time


def connect(addr):
    host, port = addr.rsplit(":", 1)
    return socket.create_connection((host, int(port)), timeout=15)


def ask_on(s, obj):
    s.sendall(b"C" + (json.dumps(obj) + "\n").encode())
    buf = b""
    while not buf.endswith(b"\n"):
        d = s.recv(65536)
        if not d:
            break
        buf += d
    return json.loads(buf or b"{}")


def ask(addr, obj):
    with connect(addr) as s:
        return ask_on(s, obj)


def pct(xs, q):
    return round(xs[min(len(xs) - 1, int(q * len(xs)))], 3) if xs else None


def reads(addr, span):
    lat, errors, last = [], 0, None
    end = time.monotonic() + span
    while time.monotonic() < end:
        sql = ["SELECT count(*) FROM t WHERE rowid = ?1", random.randint(1, 50)]
        t0 = time.monotonic()
        try:
            r = ask(addr, {"op": "get", "stmts": [sql]})
        except (OSError, ValueError) as e:
            r = {"error": str(e)}
        if "error" in r:
            errors += 1
            last = r["error"][:160]
            continue
        lat.append((time.monotonic() - t0) * 1000)
    lat.sort()
    return {"n": len(lat), "errors": errors, "last_error": last, "p50_ms": pct(lat, 0.5), "p99_ms": pct(lat, 0.99),
            "max_ms": round(lat[-1], 3) if lat else None}


def main():
    binary, port, folder, writers, seconds, after, span = sys.argv[1:8]
    klass = sys.argv[8] if len(sys.argv) > 8 else "replicated"
    port, writers, after, span = int(port), int(writers), float(after), float(span)
    peers = [f"127.0.0.1:{port + i}" for i in range(3)]
    flags = ["--deadline-s", "10", "--relay-records", "128"] + (["--spanning", "2"] if klass == "replicated" else [])
    members = [subprocess.Popen([binary, "--repl-member", "--node", str(i + 1), "--peers", ",".join(peers), "--dir",
                                 f"{folder}/m{i + 1}", "--class", klass] + flags,
                                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True) for i in range(3)]
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
    record = f"{folder}/rec.jsonl"
    driver = subprocess.Popen(
        [binary, "--repl", "commit", "--system", "amphora-procs", "--class", klass, "--members", ",".join(peers),
         "--record", record, "--batch", "1", "--text-bytes", "600", "--warmup-s", "1", "--seconds", seconds,
         "--writers", str(writers)] + flags, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, text=True)
    out = {"writers": writers, "class": klass, "leader": leader}
    if span > 0:
        time.sleep(after)
        follower = peers[leader % 3]
        out["reads_leader"] = reads(peers[leader - 1], span / 2)
        out["reads_follower"] = reads(follower, span / 2)
    _, err = driver.communicate(timeout=300)
    for m in members:
        try:
            m.wait(timeout=15)
        except subprocess.TimeoutExpired:
            m.kill()
            m.wait()
    out["driver_err_tail"] = err.strip().splitlines()[-2:]
    try:
        r = json.loads(open(record).read().strip().splitlines()[-1])
        out["commits_s"] = r.get("writes_per_s")
        out["latency_ms"] = r.get("latency_ms")
        out["refused"] = r.get("refused")
    except (OSError, IndexError, ValueError) as e:
        out["record_error"] = str(e)
    print(json.dumps(out))


if __name__ == "__main__":
    main()
