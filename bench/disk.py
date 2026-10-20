# The disk's floor for a durable write: how long a durable 4 KiB write takes on one filesystem, in
# place or appended, flushed or written through, and whether writes from several threads overlap.
# usage: python3 disk.py <dir on the filesystem> <seconds a case>
import os, sys, threading, time, statistics, subprocess

d, secs = sys.argv[1], float(sys.argv[2])
os.makedirs(d, exist_ok=True)
BLOCK = b"x" * 4096
SIZE = 64 << 20

def path(name, nocow=False):
    p = os.path.join(d, name)
    if os.path.exists(p):
        os.unlink(p)
    if nocow:
        open(p, "wb").close()
        subprocess.run(["chattr", "+C", p], check=False, capture_output=True)
    return p

def prealloc(p):
    fd = os.open(p, os.O_RDWR | os.O_CREAT, 0o644)
    # Written once (not merely allocated): later overwrites change no extents.
    chunk = b"\0" * (1 << 20)
    for _ in range(SIZE >> 20):
        os.write(fd, chunk)
    os.fsync(fd)
    os.close(fd)

def worker(p, mode, out, stop, flags=0):
    fd = os.open(p, os.O_RDWR | os.O_CREAT | flags, 0o644)
    off, lat = 0, []
    while not stop.is_set():
        t = time.perf_counter()
        if mode == "append":
            os.write(fd, BLOCK)
        else:
            os.pwrite(fd, BLOCK, off)
            off = (off + 4096) % SIZE
        if not flags:
            os.fdatasync(fd)
        lat.append(time.perf_counter() - t)
    os.close(fd)
    out.extend(lat)

def run(name, files, threads, mode, nocow=False, flags=0):
    ps = [path(f"{name}-{i}", nocow) for i in range(files)]
    if mode == "overwrite":
        for p in ps:
            prealloc(p)
    stop, outs, ts = threading.Event(), [], []
    for i in range(threads):
        o = []
        outs.append(o)
        ts.append(threading.Thread(target=worker, args=(ps[i % files], mode, o, stop, flags)))
    for t in ts: t.start()
    time.sleep(secs)
    stop.set()
    for t in ts: t.join()
    lat = sorted(x for o in outs for x in o)
    n = len(lat)
    q = lambda f: lat[min(n - 1, int(n * f))] * 1000
    print(f"{name:34s} files={files} threads={threads}: {n/secs:8.0f} writes/s  p50 {q(.5):6.2f} ms  p99 {q(.99):6.2f} ms  max {lat[-1]*1000:7.2f} ms", flush=True)
    for p in ps:
        os.unlink(p)

run("append+fdatasync", 1, 1, "append")
run("overwrite+fdatasync", 1, 1, "overwrite")
run("nocow overwrite+fdatasync", 1, 1, "overwrite", nocow=True)
run("nocow overwrite O_DSYNC", 1, 1, "overwrite", nocow=True, flags=os.O_DSYNC)
run("append+fdatasync", 2, 2, "append")
run("append+fdatasync", 4, 4, "append")
run("append+fdatasync same file", 1, 4, "append")
run("nocow overwrite+fdatasync", 4, 4, "overwrite", nocow=True)
run("nocow overwrite O_DSYNC", 4, 4, "overwrite", nocow=True, flags=os.O_DSYNC)
run("nocow overwrite O_DSYNC", 16, 16, "overwrite", nocow=True, flags=os.O_DSYNC)
