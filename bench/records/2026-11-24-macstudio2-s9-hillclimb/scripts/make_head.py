#!/usr/bin/env python3
"""Lane S9: README.md's verdict, computed from the rounds. usage: make_head.py ROUNDS_DIR"""
import os, sys
here = os.path.dirname(os.path.abspath(__file__))
src = open(os.path.join(here, "s9_tables.py")).read()
src = src[:src.rindex("\n\nmain()")]
ns = {"__name__": "t", "sys": sys}
exec(compile(src, "s9_tables.py", "exec"), ns)
load, col, med, f, mr, ratio, rt, OPS, CLIENTS, WRITES = (ns[k] for k in ("load", "col", "med", "f", "mr", "ratio", "rt", "OPS", "CLIENTS", "WRITES"))
cells, passes = load(sys.argv[1])
g = lambda op, c, arm, k="ops": col(cells.get((op, c, arm)), k)
nr = {a: len({p["round"] for p in passes if p["arm"] == a}) for a in ("A", "B", "Bd", "C", "S", "T", "Sf", "Tf")}
READS = [o for o in OPS if o not in WRITES]
R = {(op, c): ratio(g(op, c, "A"), g(op, c, "B")) for op in OPS for c in CLIENTS}
v = lambda x: sum(1 for r in R.values() if r and r["v"] == x)
def span(keys):
    xs = [R[k]["r"] for k in keys if R[k] and R[k]["r"] != float("inf")]
    return f"{min(xs):.2f} to {max(xs):.2f}x" if xs else "-"
print("# S9: perf/hillclimb (b5dd793c) against its base (8fc85651), main, SQLite and Turso, on Mac Studio 2 (2026-11-24)\n")
print('Ada, 11-24: "Examine our work on perf/hillclimb and benchmark on a studio or other free runner." Lane S9. Reads and writes of the suite\'s 17 statements at 1, 32 and 128 clients (lane S7\'s record), in process, one client code path, on the Mac Studio. What the numbers are and aren\'t is under "Durability" and "What limits the rows".\n')
print("## Verdict\n")
print(f"- **B against A:** {v('ahead')} of 51 cells ahead (B's every round above A's every round), {v('behind')} behind, {v('no measured difference')} no measured difference; A and B have {nr['A']} and {nr['B']} rounds, SQLite {nr['S']}, Turso {nr['T']}, main {nr['C']}, Bd {nr['Bd']}, Sf and Tf {nr['Sf']}.")
print(f"- **Read gains.** At 32 and 128 clients B is {span([(o, c) for o in READS for c in (32, 128) if o != 'table_count'])} A in the reads (table_count excepted); at 1 client {span([(o, 1) for o in READS if o != 'table_count'])} (writes 1 client {span([(o, 1) for o in OPS if o in WRITES])}, 32 and 128 {span([(o, c) for o in OPS if o in WRITES for c in (32, 128)])}).")
bd32 = {o: ratio(g(o, 32, "A"), g(o, 32, "Bd")) for o in READS}
xs = [r["r"] for o, r in bd32.items() if r and r["r"] != float("inf")]
bd128 = [r["r"] for r in (ratio(g(o, 128, "A"), g(o, 128, "Bd")) for o in READS) if r and r["r"] != float("inf")]
bd1 = [r["r"] for r in (ratio(g(o, 1, "A"), g(o, 1, "Bd")) for o in READS) if r]
print(f"- **Where the gain is.** Neither the defaults nor the code alone. Bd, B's code with A's four defaults put back, is {min(xs):.2f} to {max(xs):.2f}x A in the reads at 32 clients ({nr['Bd']} round), and A's code with its own shared pages and Slots switched on is 1.29x A (point_pk at 32: 571,463 against 441,758 ops/s, one pass); B with its defaults is 13.5x A. The ablation (one pass a cell, `ablate.txt`): point_pk at 32 on B is 5.93M ops/s by default, 0.70M with shared pages off, 0.59M with the Pool checkout, 6.62M with the Today page path, 6.03M with 2,000 reader pages, 6.04M with a 1,024-page writer. So the 32-client gain needs shared pages and Slots together, on B's code; the Plain page path and the writer cache don't show in it. At 128 clients Bd is {min(bd128):.2f} to {max(bd128):.2f}x A (its Pool checkout is slower than A's there). At 1 client Bd is {min(bd1):.2f} to {max(bd1):.2f}x A: that much is the code, the rest the defaults.")
tc = [R[("table_count", c)] for c in CLIENTS]
print(f"- **Regression:** table_count (COUNT(*)) is {', '.join(f'{t[chr(114)]:.2f}x at {c}' for t, c in zip(tc, CLIENTS))} clients of A, the only cells where every B round is below every A round, and 0.16 to 0.29x of Turso. The ablation names the default: with AMPHORA_READER_PAGES=2000 (2,000 private pages a reader, shared pages still on) B's table_count is 63,074 at 1 client (21,338 by default, A 61,642) and 911,773 at 32 (80,722 by default, A 265,119); with shared pages off 61,656 and 437,833. The reader cache sized from the L1 data cache (a floor of 200 pages against A's 2,000; commit aee3dae8) is what costs it: one switch, one pass each. With that default at 2,000, table_count at 32 clients would be 3.4x A and 2.3x Turso. (A with its own shared pages and Slots on answered 56 table_counts a second at 1 client, one pass: a fault of A's opt-in combination, not of B.)")
rows = []
for op in OPS:
    for c in CLIENTS:
        comp = {t: g(op, c, t) for t in (("Sf", "Tf") if op in WRITES else ("S", "T")) if g(op, c, t)}
        best = max(comp, key=lambda t: med(comp[t]), default=None)
        if best:
            rows.append((op, c, best, ratio(comp[best], g(op, c, "B")), ratio(comp[best], g(op, c, "A"))))
ten = [r for r in rows if r[3] and r[3]["r"] >= 10]
tenA = [r for r in rows if r[4] and r[4]["r"] >= 10]
beh = [r for r in rows if r[3] and r[3]["v"] == "behind"]
print(f"- **Against the best competitor (same durability):** B reaches 10x it in {len(ten)} of {len(rows)} cells (A in {len(tenA)}), and is behind it (every round) in {len(beh)}; at 1 client the read ratios are {min(r[3]['r'] for r in rows if r[1] == 1 and r[0] in READS):.2f} to {max(r[3]['r'] for r in rows if r[1] == 1 and r[0] in READS):.2f}. The 10x cells are the 32- and 128-client rows, where SQLite's and Turso's own CPU time is mostly the kernel's on macOS (below), which Linux doesn't show (lane S7): not a Linux number.")
m = {a: med([p["max_rss_mb"] for p in passes if p["arm"] == a]) for a in nr if any(p["arm"] == a for p in passes)}
print(f"- **Memory.** A whole pass, peak resident set (median MiB): " + ", ".join(f"{a} {x:,.0f}" for a, x in m.items() if x is not None) + ". B is below A over a pass and at 128, 512 and 1,024 clients (278, 321 and 350 MiB against 1,543, 1,767 and 3,054 for A), and ABOVE it at 1 client: 186 MiB against 88 (`many.jsonl`), a finding. At 512 and 1,024 clients A nearly stops: point_pk 20,769 and 127 ops/s against 2,105,561 and 1,575,234 for B (one pass).")
print("- **Correctness:** see `answers.txt` (every read answer against SQLite's of the same round) and TABLES.md 5 (errors and retries).")
