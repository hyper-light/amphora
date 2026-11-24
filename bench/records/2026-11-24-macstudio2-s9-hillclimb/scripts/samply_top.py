#!/usr/bin/env python3
"""Lane S9: top functions by self and total time from a samply profile (--save-only --unstable-presymbolicate).
usage: samply_top.py PROFILE.json.gz [THREAD_NAME_REGEX] [N]
Counts samples that used CPU (threadCPUDelta > 0); a thread that waits is sampled too and is left out."""
import bisect, collections, gzip, json, os, re, sys
path = sys.argv[1]
want = re.compile(sys.argv[2]) if len(sys.argv) > 2 and sys.argv[2] else None
N = int(sys.argv[3]) if len(sys.argv) > 3 else 25
prof = json.load((gzip.open if path.endswith(".gz") else open)(path, "rt"))
syms = {}
cand = path.replace(".json.gz", ".syms.json")
if os.path.exists(cand):
    s = json.load(open(cand))
    st = s["string_table"]
    for d in s["data"]:
        syms[d.get("debug_name")] = sorted((e["rva"], e["size"], st[e["symbol"]]) for e in d["symbol_table"])
shared = prof.get("shared", {})
libs = prof.get("libs") or []
self_c, total_c = collections.Counter(), collections.Counter()
on = alls = 0
per_thread = collections.Counter()
cache = {}


def demangle(n):
    return re.sub(r"::h[0-9a-f]{16}$", "", n)[:150]


def name_of(th, strs, fi):
    ft = th["frameTable"]
    func = ft["func"][fi]
    nm = strs[th["funcTable"]["name"][func]]
    if not nm.startswith("0x"):
        return demangle(nm)
    res = th["funcTable"]["resource"][func]
    lib = th["resourceTable"]["lib"][res] if res is not None and res >= 0 else None
    addr = ft["address"][fi]
    key = (lib, addr)
    if key in cache:
        return cache[key]
    out = nm
    if lib is not None and lib < len(libs):
        dn = libs[lib].get("debugName") or libs[lib].get("name")
        t = syms.get(dn)
        out = f"{dn}+{addr:#x}"
        if t:
            i = bisect.bisect_right(t, (addr, 1 << 62, "")) - 1
            if i >= 0 and t[i][0] <= addr < t[i][0] + (t[i][1] or (1 << 20)):
                out = demangle(t[i][2])
    cache[key] = out
    return out


for th in prof["threads"]:
    nm = th.get("name", "")
    if want and not want.search(nm):
        continue
    strs = th.get("stringArray") or shared.get("stringArray")
    sm = th["samples"]
    n = sm["length"]
    deltas = sm.get("threadCPUDelta") or [1] * n
    w = sm.get("weight") or [1] * n
    stt = th["stackTable"]
    memo = {}
    for i in range(n):
        si = sm["stack"][i]
        if si is None:
            continue
        wt = w[i] or 1
        alls += wt
        if not deltas[i]:
            continue
        fr = memo.get(si)
        if fr is None:
            fr, s0 = [], si
            while s0 is not None:
                fr.append(name_of(th, strs, stt["frame"][s0]))
                s0 = stt["prefix"][s0]
            memo[si] = fr
        on += wt
        per_thread[nm] += wt
        self_c[fr[0]] += wt
        for x in set(fr):
            total_c[x] += wt
print(f"{os.path.basename(path)}: {on} on-CPU samples of {alls}; threads by on-CPU samples {per_thread.most_common(3)}")
print("Top self (on-CPU):")
for k, v in self_c.most_common(N):
    print(f"  {100 * v / max(1, on):5.1f}%  {k}")
print("Top total (on-CPU):")
for k, v in total_c.most_common(N):
    print(f"  {100 * v / max(1, on):5.1f}%  {k}")
