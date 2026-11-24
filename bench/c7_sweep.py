"""Lane C7: runs the outer-join sweep's full matrix (db, statistics, plan, workers, statement), each
statement's runs in processes of their own, so a slow one blocks no other. Not for main.

usage: sweep.py <test binary> <out dir> <processes> <unit timeout s> <long timeout s> <deadline HH:MM[:SS]Z> <spec>...
spec: db:analyzed:plans:workers:slices   (e.g. middle:1:0-5:0-4:4)
Every run prints one line; a unit cut by its timeout is split at the statement it was on, which is run
alone with the long timeout."""
import datetime, os, subprocess, sys, threading, time, collections
binary, out, processes, unit_timeout, long_timeout, deadline = sys.argv[1:7]
processes, unit_timeout, long_timeout = int(processes), float(unit_timeout), float(long_timeout)
specs = sys.argv[7:]
now = datetime.datetime.now(datetime.timezone.utc)
parts = [int(x) for x in deadline.rstrip("Z").split(":")] + [0]
stop_at = now.replace(hour=parts[0], minute=parts[1], second=parts[2], microsecond=0)
if stop_at < now - datetime.timedelta(hours=12):
    stop_at += datetime.timedelta(days=1)
os.makedirs(out, exist_ok=True)
TEST = "translate::optimizer::bushy_search_tests::tests::c7_probe_slice"
COUNTS = {"small": 480, "small_any": 480, "middle": 240, "facts": 60}

def spread(text):
    a, _, b = text.partition("-")
    return range(int(a), int(b or a) + 1)

# unit: (db, analyzed, plan, worker, statements, long)
queue = collections.deque()
expected = set()
for spec in specs:
    db, analyzed, plans, workers, slices = spec.split(":")
    for plan in spread(plans):
        for worker in spread(workers):
            for part in range(int(slices)):
                statements = [i for i in range(COUNTS[db]) if i % int(slices) == part]
                queue.append((db, int(analyzed), plan, worker, statements, False))
                expected.update((db, int(analyzed), plan, worker, i) for i in statements)
lock = threading.Lock()
results, wrong, monsters, failures = {}, [], [], []
results_file = open(os.path.join(out, "results.tsv"), "w")
wrong_file = open(os.path.join(out, "wrong.txt"), "w")
unit_number = 0
cv = threading.Condition(lock)
running = 0

def run_unit(unit):
    global unit_number
    db, analyzed, plan, worker, statements, long = unit
    with lock:
        unit_number += 1
        number = unit_number
    log = os.path.join(out, f"unit-{number}.log")
    env = dict(os.environ, C7_DB=db, C7_ANALYZED=str(analyzed), C7_PLANS=str(plan), C7_WORKERS=str(worker),
               C7_SLICE="0/1", C7_ONLY=",".join(map(str, statements)), XDG_CACHE_HOME=os.environ.get("XDG_CACHE_HOME", ""))
    left = (stop_at - datetime.datetime.now(datetime.timezone.utc)).total_seconds()
    limit = min(long_timeout if long else unit_timeout, max(left, 1.0))
    timed_out = False
    with open(log, "wb") as handle:
        child = subprocess.Popen(["nice", "-n", "10", binary, "--ignored", "--exact", TEST, "--nocapture", "--test-threads=1"],
                                 stdout=handle, stderr=subprocess.STDOUT, env=env)
        try:
            code = child.wait(timeout=limit)
        except subprocess.TimeoutExpired:
            timed_out = True
            child.kill()
            code = child.wait()
    text = open(log, "rb").read().decode("utf-8", "replace")
    begun, done = None, set()
    with lock:
        for line in text.splitlines():
            f = line.split("\t")
            if f[0] == "C7BEGIN":
                begun = int(f[5])
            elif f[0] == "C7RUN":
                key = (f[1], int(f[2]), int(f[3]), int(f[4]), int(f[5]))
                results[key] = (int(f[6]), f[7], f[8])
                done.add(int(f[5]))
                results_file.write(line + "\n")
            elif f[0] == "C7WRONG":
                wrong.append(line)
                wrong_file.write(line + "\n")
        results_file.flush(); wrong_file.flush()
        left_over = [i for i in statements if i not in done]
        if left_over:
            if timed_out and begun is not None and begun not in done:
                monsters.append((db, analyzed, plan, worker, begun))
                if long:
                    failures.append((unit, "long run cut"))
                else:
                    queue.append((db, analyzed, plan, worker, [begun], True))
                    rest = [i for i in left_over if i != begun]
                    if rest:
                        queue.appendleft((db, analyzed, plan, worker, rest, False))
            else:
                failures.append((unit, f"exit {code}, {len(left_over)} statements not run; log {log}"))
    return code

def loop():
    global running
    while True:
        with cv:
            while not queue and running:
                cv.wait()
            if not queue:
                return
            if datetime.datetime.now(datetime.timezone.utc) >= stop_at:
                return
            unit = queue.popleft()
            running += 1
        try:
            run_unit(unit)
        finally:
            with cv:
                running -= 1
                cv.notify_all()

began = time.monotonic()
threads = [threading.Thread(target=loop) for _ in range(processes)]
for t in threads: t.start()
for t in threads: t.join()
took = time.monotonic() - began
missing = sorted(expected - set(results))
by = collections.Counter()
slow = sorted(((r[0], k, r[2]) for k, r in results.items()), reverse=True)[:8]
print(f"binary {binary}; took {took:.0f} s at {processes} processes; load {os.getloadavg()}")
print(f"expected {len(expected)} runs, done {len(set(results) & expected)}, missing {len(missing)}, wrong {sum(1 for r in results.values() if r[1] != 'ok')}, wrong lines {len(wrong)}")
for db, analyzed in sorted({(k[0], k[1]) for k in expected}):
    e = {k for k in expected if (k[0], k[1]) == (db, analyzed)}
    d = e & set(results)
    bad = [k for k in d if results[k][1] != "ok"]
    ms = [results[k][0] for k in d]
    print(f"{db} analyzed={analyzed}: {len(d)} of {len(e)} runs, {len(bad)} wrong, {sum(ms)/1000:.0f} s of query time, slowest {max(ms, default=0)} ms, "
          f"bushy {sum('bushy=0 ' not in results[k][2] + ' ' for k in d)}, hooked {sum('hooked=true' in results[k][2] for k in d)}")
for ms, key, _ in slow: print("slow", ms, key)
for m in monsters: print("monster", m)
for f in failures[:20]: print("FAIL", f)
print("missing first 20:", missing[:20])
for line in wrong: print("WRONG", line[:400])
sys.exit(1 if (wrong or missing or failures) else 0)
