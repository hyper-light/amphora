#!/usr/bin/env python3
"""Lane S9: top functions by self and total on-CPU time from a samply profile, names by atos on the profiled binary.
usage: sym_top.py PROFILE.json.gz BINARY [N]"""
import collections, gzip, json, os, re, subprocess, sys
path, binary = sys.argv[1], sys.argv[2]
N = int(sys.argv[3]) if len(sys.argv) > 3 else 14
prof = json.load(gzip.open(path, "rt"))
libs = prof.get("libs") or []
bn = os.path.basename(binary)
uniq = set()
stacks = []
on = 0
for th in prof["threads"]:
    sm, stt, ft = th["samples"], th["stackTable"], th["frameTable"]
    deltas = sm.get("threadCPUDelta") or [1] * sm["length"]
    memo = {}
    for i in range(sm["length"]):
        si = sm["stack"][i]
        if si is None or not deltas[i]:
            continue
        fr = memo.get(si)
        if fr is None:
            fr, s0 = [], si
            while s0 is not None:
                f = stt["frame"][s0]
                res = th["funcTable"]["resource"][ft["func"][f]]
                lib = libs[th["resourceTable"]["lib"][res]].get("debugName") if res is not None and res >= 0 else None
                fr.append((lib, ft["address"][f]))
                s0 = stt["prefix"][s0]
            memo[si] = fr
            uniq.update(a for l, a in fr if l == bn)
        on += 1
        stacks.append(fr)
addrs = sorted(uniq)
names = {}
for k in range(0, len(addrs), 1500):
    chunk = addrs[k:k + 1500]
    out = subprocess.run(["atos", "-o", binary, "-arch", "arm64", "-l", "0x100000000"] + [hex(0x100000000 + a) for a in chunk], capture_output=True, text=True).stdout.splitlines()
    for a, line in zip(chunk, out):
        names[a] = re.sub(r" \(in .*", "", line)[:120]
def short(lib, a):
    if lib == bn:
        n = names.get(a, hex(a))
        n = re.sub(r"^_R", "_R", n)
        return n
    return f"{lib}"
self_c, total_c = collections.Counter(), collections.Counter()
for fr in stacks:
    fs = [short(l, a) for l, a in fr]
    self_c[fs[0]] += 1
    for x in set(fs):
        total_c[x] += 1
print(f"{os.path.basename(path)}: {on} on-CPU samples; names by atos on {bn}")
print("Top self (on-CPU):")
for k, v in self_c.most_common(N):
    print(f"  {100 * v / max(1, on):5.1f}%  {k}")
print("Top total (on-CPU):")
for k, v in total_c.most_common(N + 8):
    print(f"  {100 * v / max(1, on):5.1f}%  {k}")
