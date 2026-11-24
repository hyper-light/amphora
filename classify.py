"""Groups a sweep's wrong runs (default setting only) by what differs. usage: classify.py LOG MAX"""
import re, sys, collections
text = open(sys.argv[1], errors="replace").read()
runs = re.split(r"(?=^WRONG case )", text, flags=re.M)
groups = collections.OrderedDict()
by_setting = collections.Counter()
for run in runs:
    m = re.match(r"WRONG case (\d+), (\w+):\n", run)
    if not m: continue
    by_setting[m.group(2)] += 1
    if m.group(2) != "default": continue
    lines = run.split("\n")[1:]
    i = next((k for k, l in enumerate(lines) if l.startswith("    SQLite  ")), None)
    if i is None:
        key = "PANIC"
    else:
        sql, want, got = lines[i-1].strip(), lines[i][12:], lines[i+1][12:] if i + 1 < len(lines) else "?"
        if got.startswith("refused") or want.startswith("refused"):
            key = "REFUSAL  SQLite " + re.sub(r"\d+", "#", want)[:70] + " | Amphora " + re.sub(r"\d+", "#", got)[:90]
        else:
            key = "ROWS " + sql.split(" FROM ")[-1][:20]
    groups.setdefault(key, []).append((m.group(1), [l for l in lines if l.startswith("  ")]))
print("wrong by setting:", dict(by_setting))
n = 0
for key, cases in sorted(groups.items(), key=lambda kv: -len(kv[1])):
    print(f"--- {len(cases)} x {key}")
    cases.sort(key=lambda c: sum(len(l) for l in c[1]))
    for index, lines in cases[: 3 if key.startswith("ROWS") else 1]:
        n += 1
        if n > int(sys.argv[2]): break
        print(f"  case {index}")
        for l in lines: print("   ", l[:420])
