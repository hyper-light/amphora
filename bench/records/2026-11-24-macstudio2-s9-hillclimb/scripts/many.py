#!/usr/bin/env python3
"""Lane S9: many connections: point reads, inserts and a read-write transaction, each on its own at 1, 128, 512 and 1,024
clients, on A and B, with the process's peak resident set (ru_maxrss). usage: many.py OUT.jsonl [ARMS]"""
import json, os, resource, subprocess, sys, threading, time
S9 = os.path.expanduser("~/.cache/amphora-s9")
out = sys.argv[1]
arms = sys.argv[2].split(",") if len(sys.argv) > 2 else ["A", "B"]
soft, hard = resource.getrlimit(resource.RLIMIT_NOFILE)
resource.setrlimit(resource.RLIMIT_NOFILE, (61440, hard))
for c in (1, 128, 512, 1024):
    for arm in arms if c % 512 else arms[::-1]:
        e = {k: v for k, v in os.environ.items() if not k.startswith(("AMPHORA_", "DYLD_", "KILN_"))}
        e["XDG_CACHE_HOME"] = S9 + "/xdg"
        t0 = time.time()
        p = subprocess.Popen([f"{S9}/bin/amphora-bench-{arm.lower()}", "--target", "amphora", "--dir", f"{S9}/data/many-{arm}", "--suite", "--only",
                              "point_pk$,insert_one$,txn_read_write$", "--items", "200000", "--clients", str(c), "--seconds", "1", "--warmup", "0.5"],
                             env=e, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        got = {}
        ts = [threading.Thread(target=lambda: got.__setitem__("o", p.stdout.read())), threading.Thread(target=lambda: got.__setitem__("e", p.stderr.read()))]
        [t.start() for t in ts]
        late = threading.Timer(60, p.terminate)
        late.start()
        _, status, ru = os.wait4(p.pid, 0)
        late.cancel()
        [t.join() for t in ts]
        rec = {"arm": arm, "clients": c, "took_s": round(time.time() - t0, 1), "rc": os.waitstatus_to_exitcode(status), "max_rss_mb": round(ru.ru_maxrss / 2**20, 1)}
        try:
            rep = json.loads(got["o"].strip().splitlines()[-1])
            rec["heap_peak_mb"] = rep.get("heap_peak_mb")
            rec["results"] = [[x["op"], round(x["ops_per_s"]), (x["ms"] or {}).get("p50"), (x["ms"] or {}).get("p99"), x["errors"], x["retries"], x["error"]] for x in rep["results"]]
        except Exception as x:
            rec["error"] = f"{x}: {got.get('e', '')[-500:]}"
        with open(out, "a") as f:
            f.write(json.dumps(rec) + "\n")
        print(rec["arm"], c, rec["rc"], rec["max_rss_mb"], rec.get("heap_peak_mb"), rec.get("results") or rec.get("error"), flush=True)
