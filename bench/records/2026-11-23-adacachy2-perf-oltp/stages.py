"""Lane W2 (oltp): per-insert stages from stages.c's records, anchored on each flush of the log.

python3 -I stages.py <records> <amphora|sqlite>
Cycles are the flushes of the log (fdatasync or fsync on a "-wal" file) in the longest run whose
gaps stay under 100 ms (the insert loop), less its first and last 5%. Prints the median, p90 and
mean of each stage over those cycles, in microseconds.
"""
import bisect
import collections
import statistics
import struct
import sys

PWRITE, WRITE, PWRITEV, FDATASYNC, FSYNC, OPEN, CLOSE, FUTEX, SYNC_RANGE = range(1, 10)
WAITS, WAKES = {0, 9}, {1, 10}
REC = struct.Struct("<QQqqqih10s")


def load(path):
    raw = open(path, "rb").read()
    recs = [REC.unpack_from(raw, i) for i in range(0, len(raw) - REC.size + 1, REC.size)]
    recs.sort(key=lambda r: r[0])
    names, out = {}, []
    for t0, t1, ret, a, b, tid, kind, tail in recs:
        tail = tail.rstrip(b"\0").decode(errors="replace")
        if kind == OPEN and ret >= 0:
            names[ret] = tail
        name = names.get(a, "") if kind in (PWRITE, WRITE, PWRITEV, FDATASYNC, FSYNC, CLOSE, SYNC_RANGE) else tail
        out.append((t0, t1, ret, a, b, tid, kind, name))
    return out


def loop_window(syncs):
    best, start = (0, 0), 0
    for i in range(1, len(syncs) + 1):
        if i == len(syncs) or syncs[i][0] - syncs[i - 1][0] > 100_000_000:
            if i - start > best[1] - best[0]:
                best = (start, i)
            start = i
    lo, hi = best
    cut = (hi - lo) // 20
    return syncs[lo + cut:hi - cut]


class Thread:
    def __init__(self, evs):
        self.evs = evs
        self.t0 = [e[0] for e in evs]
        self.t1 = [e[1] for e in evs]

    def first_after(self, t, pred):
        for e in self.evs[bisect.bisect_left(self.t0, t):]:
            if pred(e):
                return e
        return None

    def last_before(self, t, pred, key=1):
        i = bisect.bisect_right(self.t1 if key == 1 else self.t0, t)
        for e in reversed(self.evs[:i]):
            if pred(e):
                return e
        return None


def is_wal(e):
    return e[7].endswith("-wal")


def summary(rows):
    print(f"cycles {len(rows)}")
    for k in rows[0]:
        v = [r[k] for r in rows if r[k] is not None]
        if v:
            v.sort()
            print(f"  {k:28s} p50 {statistics.median(v) / 1e3:9.1f}  p90 {v[int(len(v) * 0.9)] / 1e3:9.1f}  mean {statistics.fmean(v) / 1e3:9.1f}  n {len(v)}")


def main():
    evs, side = load(sys.argv[1]), sys.argv[2]
    syncs = [e for e in evs if e[6] in (FDATASYNC, FSYNC) and is_wal(e)]
    by_tid = collections.defaultdict(list)
    for e in evs:
        by_tid[e[5]].append(e)
    th = {t: Thread(v) for t, v in by_tid.items()}
    win = loop_window(syncs)
    if len(win) < 3:
        print("no insert loop found", len(syncs), "log flushes")
        return
    kinds = collections.Counter((e[6], e[7] if e[6] != FUTEX else e[3]) for e in evs if win[0][0] <= e[0] <= win[-1][1])
    print(f"{side}: {len(win)} log flushes in the loop; calls in it by (kind, file or futex op): {dict(kinds.most_common(14))}")
    flusher = collections.Counter(e[5] for e in win).most_common(1)[0][0]
    wal_writes = [e for e in evs if e[6] in (PWRITE, WRITE, PWRITEV) and is_wal(e) and win[0][0] <= e[0] <= win[-1][1]]
    writer = collections.Counter(e[5] for e in wal_writes).most_common(1)[0][0] if wal_writes else flusher
    F, W = th[flusher], th[writer]
    rows = []
    for prev, s in zip(win, win[1:]):
        r = {"cycle (flush to flush)": s[1] - prev[1], "fdatasync": s[1] - s[0]}
        writes = [e for e in W.evs[bisect.bisect_left(W.t0, prev[1]):bisect.bisect_left(W.t0, s[0])] if e[6] in (PWRITE, WRITE, PWRITEV) and is_wal(e)]
        r["log writes (calls)"] = len(writes) * 1000
        r["log write bytes / 1000"] = sum(max(e[2], 0) for e in writes)
        r["log write (syscalls)"] = sum(e[1] - e[0] for e in writes) if writes else None
        if side == "sqlite":
            r["prev flush end -> 1st log write"] = writes[0][0] - prev[1] if writes else None
            r["1st log write -> last write end"] = writes[-1][1] - writes[0][0] if writes else None
            r["last write end -> fdatasync"] = s[0] - writes[-1][1] if writes else None
            rows.append(r)
            continue
        op = F.last_before(s[0], lambda e: e[6] == OPEN and is_wal(e), key=0)
        r["open of the log"] = op[1] - op[0] if op and op[0] > prev[1] else None
        cl = F.first_after(s[1], lambda e: e[6] == CLOSE)
        r["close"] = cl[1] - cl[0] if cl else None
        wake_client = F.first_after(s[1], lambda e: e[6] == FUTEX and e[3] in WAKES)
        woke = F.last_before(op[0] if op else s[0], lambda e: e[6] == FUTEX and e[3] in WAITS)
        r["flusher woke -> open"] = (op[0] - woke[1]) if (op and woke and woke[1] > prev[1]) else None
        r["fdatasync end -> wake client"] = wake_client[0] - s[1] if wake_client else None
        last_w = writes[-1] if writes else None
        if last_w:
            r["last log write end -> flusher woke"] = (woke[1] - last_w[1]) if (woke and woke[1] > last_w[1]) else None
            r["last log write end -> fdatasync"] = s[0] - last_w[1]
            w_woke = W.last_before(writes[0][0], lambda e: e[6] == FUTEX and e[3] in WAITS)
            r["writer woke -> 1st log write (apply)"] = writes[0][0] - w_woke[1] if (w_woke and w_woke[1] > prev[1]) else None
        # The client: the thread a flusher's wake reaches first after the flush.
        if wake_client:
            nxt = [(t.first_after(wake_client[0], lambda e: e[6] == FUTEX and e[3] in WAITS and e[1] > wake_client[0]), tid) for tid, t in th.items() if tid not in (flusher, writer)]
            nxt = [(e[1], tid) for e, tid in nxt if e]
            if nxt:
                t_ans, ctid = min(nxt)
                r["wake client -> client runs"] = t_ans - wake_client[0]
                r["client tid"] = None
        rows.append(r)
    summary(rows)
    print(f"  threads: flusher {flusher}, writer {writer}")


main()
