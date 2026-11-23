import re, sys
rows = []
for l in open(sys.argv[1]):
    t, kind = l.split()[:2]
    load = float(re.search(r"load=([\d.]+)", l).group(1))
    cpu = float(re.search(r"cpu\[some avg10=([\d.]+)", l).group(1))
    io = float(re.search(r"io\[some avg10=([\d.]+)", l).group(1))
    iof = float(re.search(r"iofull\[full avg10=([\d.]+)", l).group(1))
    rows.append((t, kind, load, cpu, io, iof))
phase = None; acc = {}
for t, kind, load, cpu, io, iof in rows:
    if kind.endswith("-start"): phase = kind[:-6]; acc[phase] = [t, None, []]
    elif kind.endswith("-end") or kind == "build-end": acc[phase][1] = t; acc[phase][2].append((load, cpu, io, iof)); phase = None; continue
    if phase: acc[phase][2].append((load, cpu, io, iof))
for p, (s, e, v) in acc.items():
    if not v: continue
    f = lambda i: f"{min(x[i] for x in v):.1f}-{max(x[i] for x in v):.1f} (median {sorted(x[i] for x in v)[len(v)//2]:.1f})"
    print(f"| {p} | {s} | {e or 'running'} | {len(v)} | {f(0)} | {f(1)} | {f(2)} | {f(3)} |")
