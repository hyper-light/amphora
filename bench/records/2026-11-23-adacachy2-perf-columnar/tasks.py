import sys, collections
last = {}; first = {}; clk = 100.0
for line in open(sys.argv[1], errors="replace"):
    try:
        head, stat = line.split("| ", 1)
        hp = head.split(); ts = float(hp[0]); tid = hp[1]; vol = int(hp[2]) if len(hp) > 2 else 0; inv = int(hp[3]) if len(hp) > 3 else 0
        r = stat[stat.rindex(")") + 2:].split(); comm = stat[stat.index("(") + 1:stat.rindex(")")]
        ut, st = int(r[11]), int(r[12])
    except Exception:
        continue
    if tid not in first: first[tid] = (ts, ut + st, vol, inv, comm)
    last[tid] = (ts, ut + st, vol, inv, comm)
rows = []
for tid, (ts, cpu, vol, inv, comm) in last.items():
    f = first[tid]
    rows.append((cpu - f[1], tid, comm, ts - f[0], vol - f[2], inv - f[3]))
rows.sort(reverse=True)
print("threads seen", len(rows))
for cpu, tid, comm, span, vol, inv in rows[:40]:
    print(f"tid {tid} {comm:16s} cpu {cpu*10:7.0f} ms  seen {span*1000:7.0f} ms  vol_cs {vol:6d}  invol_cs {inv:6d}")
