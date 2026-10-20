# Reads bench records (bench/records/*.jsonl, run.sh's lines) and prints them as one markdown table
# per number of writers: each target's writes a second, write latency, read latency and memory, and
# Amphora's ratio to each (how many times better: higher throughput, lower latency and memory). A
# target run more than once shows its best run by writes a second, and how many runs it had. A
# server's memory is what its processes hold (anon + shmem, run.sh's server_mem_held_peak_mb), the
# figure to set beside a process's resident set; its cgroup's memory.peak counts the page cache too.
# usage: python3 table.py <records.jsonl> [more.jsonl ...]
import json, sys, collections

rows = collections.defaultdict(dict)
for path in sys.argv[1:]:
    for line in open(path):
        line = line.strip()
        if not line.startswith("{"):
            continue
        r = json.loads(line)
        if r.get("failed"):
            rows[r["writers"]].setdefault(r["target"], {"failed": r.get("error", "")[-120:], "runs": 0})
            continue
        d = r["report"]
        run = {
            "load": r.get("load_before"),
            "w/s": d.get("write_events_per_s") or 0,
            "w": d.get("write_ms") or {},
            "r": d.get("read_ms") or {},
            "mem": r.get("server_mem_held_peak_mb") or r.get("server_mem_peak_mb") or d.get("rss_anon_max_mb") or d.get("rss_max_mb"),
        }
        have = rows[d["writers"]].get(d["target"])
        runs = (have or {}).get("runs", 0) + 1
        if have is None or "failed" in have or run["w/s"] > have["w/s"]:
            have = run
        rows[d["writers"]][d["target"]] = dict(have, runs=runs)

def ms(h, q):
    v = (h or {}).get(q)
    return "-" if v is None else (f"{v:.3f}" if v < 10 else f"{v:.0f}")

def ratio(a, b, higher_better):
    if not a or not b:
        return ""
    x = (a / b) if higher_better else (b / a)
    return f"{x:.1f}x"

for writers in sorted(rows):
    targets = rows[writers]
    amp = targets.get("amphora", {})
    print(f"\n### {writers:,} writers\n")
    print("| target | runs | load | writes/s | write p50 | p99 | p99.9 | max | read p50 | read p99 | memory MB | Amphora: throughput | p99 | p99.9 | max | memory |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    for t, v in sorted(targets.items(), key=lambda kv: (kv[0] != "amphora", kv[0])):
        if "failed" in v:
            print(f"| {t} | | | failed: {v['failed']} | | | | | | | | | | | | |")
            continue
        w, r = v["w"], v["r"]
        cells = [t, f"{v['runs']}", f"{v['load']}", f"{v['w/s']:,.0f}", ms(w, "p50"), ms(w, "p99"), ms(w, "p999"), ms(w, "max"), ms(r, "p50"), ms(r, "p99"), f"{v['mem']}"]
        if t != "amphora" and "failed" not in amp and amp:
            aw = amp["w"]
            cells += [ratio(amp["w/s"], v["w/s"], True), ratio(aw.get("p99"), w.get("p99"), False), ratio(aw.get("p999"), w.get("p999"), False), ratio(aw.get("max"), w.get("max"), False), ratio(amp["mem"], v["mem"], False)]
        else:
            cells += ["", "", "", "", ""]
        print("| " + " | ".join(cells) + " |")
