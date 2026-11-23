import sys, collections
path = sys.argv[1]; top = int(sys.argv[2]) if len(sys.argv) > 2 else 25
idle_marks = ("futex", "syscall", "epoll", "nanosleep", "clock_nanosleep", "pthread_cond", "__GI___poll", "wait4", "pause", "do_futex", "park", "std::sys::pal::unix::thread::Thread::sleep", "recv", "read ()")
samples = []; cur = []
for line in open(path, errors="replace"):
    line = line.rstrip("\n")
    if line == "--":
        samples.append(cur); cur = []; continue
    parts = line.split("|", 2)
    if len(parts) == 3: cur.append(parts)
threads_per = collections.Counter(); busy_per = collections.Counter()
incl = collections.Counter(); leaf = collections.Counter(); leaf2 = collections.Counter(); names = collections.Counter()
busy_total = 0
for s in samples:
    threads_per[len(s)] += 1
    nb = 0
    for num, name, stack in s:
        frames = stack.split(";")
        lf = frames[-1] if frames else "?"
        is_idle = any(m in lf for m in idle_marks) or lf in ("??",) and any(m in stack for m in ("futex", "park", "wait"))
        names[(name, "idle" if is_idle else "busy")] += 1
        if is_idle: continue
        nb += 1; busy_total += 1
        for f in set(frames): incl[f] += 1
        leaf[lf] += 1
        leaf2[" <- ".join(reversed(frames[-3:]))] += 1
    busy_per[nb] += 1
print("samples", len(samples), "threads/sample", dict(sorted(threads_per.items())), "busy threads/sample", dict(sorted(busy_per.items())))
print("thread states", dict(names.most_common(12)))
print("busy thread-samples", busy_total)
print("== inclusive (share of busy thread-samples)")
for f, c in incl.most_common(top): print(f"{100*c/max(busy_total,1):5.1f}% {f[:150]}")
print("== leaf")
for f, c in leaf.most_common(top): print(f"{100*c/max(busy_total,1):5.1f}% {f[:150]}")
print("== leaf <- caller <- caller")
for f, c in leaf2.most_common(15): print(f"{100*c/max(busy_total,1):5.1f}% {f[:220]}")
