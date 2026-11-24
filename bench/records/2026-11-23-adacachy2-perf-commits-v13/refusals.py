#!/usr/bin/env python3
"""Lane V13: each write refused for a stale base at the leader's append, traced across the three
members' AMPHORA_TRACE13 logs: its barrier, its slot, its prepare base, its forward's arrival, and the
entries between its base and its append (whose, and the slot as each appended). Ends with the count
of each first wrong decision.   refusals.py OUT_DIR
"""
import collections, glob, os, re, sys

out = sys.argv[1]
ev = []
for path in sorted(glob.glob(os.path.join(out, "trace-m*.log"))):
    m = int(re.search(r"trace-m(\d+)", path).group(1))
    for line in open(path, errors="replace"):
        parts = line.rstrip("\n").split(" ", 2)
        if len(parts) >= 2 and parts[0].isdigit():
            ev.append((int(parts[0]), m, parts[1], parts[2] if len(parts) > 2 else ""))
ev.sort()
if not ev:
    print("no trace"); sys.exit(1)
t0 = ev[0][0]
ms = lambda ns: f"{(ns - t0) / 1e6:9.2f}"
kv = lambda s, k: (re.search(r"(?:^| )" + k + r"=([^ \]]*)", s) or [None, None])[1]

by_ctx = collections.defaultdict(list)
for e in ev:
    c = kv(e[3], "ctx")
    if c:
        by_ctx[c[:64]].append(e)
apps = [e for e in ev if e[2] == "app"]
stale = [e for e in apps if kv(e[3], "stale") == "true"]
leader_events = [e for e in ev if e[2] in ("start", "end", "fwd", "bar", "ready", "app", "own", "det")]
print(f"{out}: {len(apps)} forwards appended at a leader, {len(stale)} stale; "
      f"res ser at members: {sum(1 for e in ev if e[2] == 'res' and kv(e[3], 'ser') == 'true' and kv(e[3], 'mine') == 'true')}; "
      f"detector verdicts: {sum(1 for e in ev if e[2] == 'det')}; slot ends: "
      + str(collections.Counter((kv(e[3], 'site'), kv(e[3], 'how')) for e in ev if e[2] == 'end'))
      + "; bar/fwd ended: " + str(collections.Counter(re.search(r"ended=(None|Some\(\(.*?\)\))", e[3]).group(1) for e in ev if e[2] in ("bar", "fwd") and "ended=" in e[3])))

verdicts = collections.Counter()
for a in stale:
    ns, lm, _, s = a
    ctx = kv(s, "ctx")[:64]
    frm = int(kv(s, "from"))
    life = by_ctx.get(ctx, [])
    print(f"\n== refused ctx={ctx[:20]}.. from={frm} idx={kv(s, 'idx')} base={kv(s, 'base')} before={kv(s, 'before')} at {ms(ns)} ms (leader m{lm})")
    for e in life:
        print(f"   {ms(e[0])} m{e[1]} {e[2]:5} {e[3][:260]}")
    bar = next((e for e in life if e[2] == "bar" and e[1] == lm), None)
    fwd = next((e for e in life if e[2] == "fwd" and e[1] == lm), None)
    if bar is None or fwd is None:
        verdicts["barrier or forward not seen at this leader"] += 1
        continue
    rdy = next((e for e in life if e[2] == "ready" and e[1] == lm and kv(e[3], "proposal") == "false"), None)
    stepped_at = bar[0] if kv(bar[3], "step") == "True" or kv(bar[3], "step") == "true" else (rdy[0] if rdy else None)
    window = [e for e in leader_events if e[1] == lm and bar[0] <= e[0] <= a[0] and (
              (e[2] in ("start", "end") and kv(e[3], "member") == str(frm)) or e[2] == "det"
              or (e[2] in ("bar", "fwd") and f"ended=Some(({frm}," in e[3] and kv(e[3], "ctx")[:64] != ctx))]
    for e in window:
        print(f"   {ms(e[0])} m{e[1]} {e[2]:5} {e[3][:260]}")
    between = re.search(r"between=\[(.*)\]$", s)
    entries = between.group(1).split() if between else []
    print("   between:")
    for item in entries:
        idx, origin, kind, ectx = item.split(":", 3)
        who = next((e for e in by_ctx.get(ectx[:64], []) if e[1] == lm and e[2] in ("app", "own")), None)
        slot = re.search(r"slot\[([^\]]*)\]", who[3]).group(1) if who else "?"
        print(f"     idx={idx} origin={origin} {kind} via={who[2] if who else '?'} at={ms(who[0]) if who else '?'} slot[{slot}]")
    if kv(bar[3], "step") in ("True", "true"):
        verdicts[f"barrier stepped at once (marked={kv(bar[3], 'marked')}, ended={re.search(r'ended=(None|Some\(\(.*?\)\))', bar[3]).group(1)})"] += 1
    elif rdy is None:
        verdicts["barrier never stepped from the slot at this leader"] += 1
    else:
        began = [e for e in window if e[2] == "start" and kv(e[3], "member") == str(frm) and e[0] <= rdy[0]]
        ends = [e for e in window if e[2] in ("end", "bar", "fwd") and e[0] >= rdy[0] and e[0] <= fwd[0]
                and (e[2] != "end" or kv(e[3], "member") == str(frm))]
        fwd_ended = re.search(r"ended=(None|Some\(\(.*?\)\))", fwd[3]).group(1)
        if not began:
            verdicts["barrier stepped by an end, with no slot of its own"] += 1
        elif ends:
            e0 = ends[0]
            site = kv(e0[3], 'site') if e0[2] == "end" else f"{e0[2]} from m{kv(e0[3], 'from')}"
            how = kv(e0[3], 'how') if e0[2] == "end" else re.search(r"ended=(None|Some\(\(.*?\)\))", e0[3]).group(1)
            verdicts[f"slot ended before its forward: site={site} how={how}"] += 1
        else:
            holder_at_fwd = re.search(r"before\[holder=(\S+)", fwd[3]).group(1)
            verdicts[f"forward arrived with holder={holder_at_fwd.split('@')[0] == str(frm) and 'itself' or holder_at_fwd}, fwd ended={fwd_ended}"] += 1
print("\nfirst wrong decision, counted:")
for k, v in verdicts.most_common():
    print(f"  {v:4}  {k}")

print("\nthe leaders' own writes refused at apply:")
own_verdicts = collections.Counter()
for r in ev:
    if r[2] != "res" or kv(r[3], "ser") != "true" or kv(r[3], "mine") != "true":
        continue
    ctx = kv(r[3], "ctx")[:64]
    own = next((e for e in by_ctx.get(ctx, []) if e[2] == "own" and e[1] == r[1]), None)
    if own is None or kv(own[3], "leader") != "true":
        continue
    base, last = kv(own[3], "base"), kv(own[3], "last")
    print(f"== m{r[1]} own ctx={ctx[:20]}.. idx={kv(r[3], 'idx')} at {ms(r[0])}")
    for e in by_ctx.get(ctx, []):
        print(f"   {ms(e[0])} m{e[1]} {e[2]:5} {e[3][:300]}")
    lo = own[0] - 200_000_000
    for e in ev:
        if e[1] == r[1] and lo <= e[0] <= own[0] and e[2] in ("app", "own", "start", "end", "fwd", "ready") and kv(e[3], "ctx") != kv(own[3], "ctx"):
            print(f"   {ms(e[0])} m{e[1]} {e[2]:5} {e[3][:200]}")
    own_verdicts[f"own prepared on base={base} with last={last}: " + ("base behind the log" if base and last and base != f"Some({last})" else "base at the log's last")] += 1
for k, v in own_verdicts.most_common():
    print(f"  {v:4}  {k}")
