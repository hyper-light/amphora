import subprocess, sys, os, concurrent.futures as cf
exe, tsv, setting, limit = sys.argv[1], sys.argv[2], sys.argv[3], float(sys.argv[4])
O = os.path.expanduser("~/.cache/amphora-perf/vdbe-c24-out/chunks"); os.makedirs(O, exist_ok=True)
dbs = []
for line in open(tsv):
    line = line.rstrip("\n")
    if line.startswith("db\t"): dbs.append((line, [], []))
    elif line.startswith("sql\t"): dbs[-1][1].append(line)
    elif line.startswith("q\t"): dbs[-1][2].append(line)
def run(args):
    i, head, sql, qs = args
    f = f"{O}/{os.getpid()}-{setting}-{i}.tsv"
    open(f, "w").write("\n".join([head] + sql + qs) + "\n")
    env = dict(os.environ, C24_SQL=f, C24_ONE=setting)
    try:
        p = subprocess.run([exe, "--nocapture", "--test-threads=1"], env=env, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, timeout=limit)
        return (qs, p.stdout, False)
    except subprocess.TimeoutExpired: return (qs, "", True)
    finally: os.unlink(f)
def jobs(size, sets):
    n = 0
    for head, sql, qs in sets:
        for j in range(0, len(qs), size):
            n += 1; yield (n, head, sql, qs[j:j+size])
hung = []; differ = 0; total = 0; out = []
with cf.ThreadPoolExecutor(int(os.environ.get("W", "8"))) as ex:
    res = list(ex.map(run, jobs(25, dbs)))
    again = []
    for (qs, text, late), job in zip(res, jobs(25, dbs)):
        if late: again.append((job[1], job[2], job[3]))
        else: out.append(text)
    res2 = list(ex.map(run, jobs(1, again)))
    for qs, text, late in res2:
        if late: hung.append(qs[0])
        else: out.append(text)
text = "\n".join(out)
import re
for t in out:
    for m in re.finditer(r"== \[[^\]]*\] (\d+) of (\d+) differ", t):
        differ += int(m.group(1)); total += int(m.group(2))
print(f"## {os.path.basename(exe)} {setting}: {differ} of {total} differ, {len(hung)} hung")
for h in hung[:int(os.environ.get("HUNG", "8"))]: print("HUNG", h[:500])
if os.environ.get("SHOW"):
    shown = 0; lines = text.splitlines()
    for k, l in enumerate(lines):
        if l.lstrip().startswith("[") or "... [" in l:
            print("\n".join(x[:420] for x in lines[k:k+3])); shown += 1
            if shown >= int(os.environ["SHOW"]): break
