#!/usr/bin/env python3
"""Lane Q2: reads the cache censuses AMPHORA_CACHE_CENSUS wrote (<pager>-b<n>.txt before count n,
<pager>-a<n>.txt after it) and prints, per pager in the order its first census was written, what
each count read from the file and evicted, which pages it evicted (its own or older ones), where
the clock hand stood, and the reference levels on each side of it. usage: q2_census.py DIR"""
import collections, os, re, sys

d = sys.argv[1]


def load(path):
    lines = open(path).read().splitlines()
    head = dict(zip(lines[0].split()[::2], map(int, lines[0].split()[1::2])))
    head.update(dict(zip(lines[1].split()[::2], map(int, lines[1].split()[1::2]))))
    pages = []
    for line in lines[3:]:
        f = list(map(int, line.split()))
        pages.append(dict(zip(["idx", "pgno", "ref", "hits", "born", "evictable", "dirty", "locked", "pinned", "strong", "shared"], f)))
    return head, pages


def refs(pages):
    c = collections.Counter(p["ref"] for p in pages)
    return "/".join(str(c.get(i, 0)) for i in range(4))


files = collections.defaultdict(dict)
for name in os.listdir(d):
    m = re.match(r"([0-9a-f]+)-([ab])(\d+)\.txt$", name)
    if m:
        files[m.group(1)][(m.group(2), int(m.group(3)))] = os.path.join(d, name)
order = sorted(files, key=lambda k: min(os.path.getmtime(p) for p in files[k].values()))
print("refs = pages at reference level 0/1/2/3; ahead = from the hand to the back (newest), behind = the rest")
for k in order:
    for n in range(4):
        b, a = files[k].get(("b", n)), files[k].get(("a", n))
        if not b or not a:
            continue
        hb, pb = load(b)
        ha, pa = load(a)
        before_keys = {p["pgno"] for p in pb}
        new = [p for p in pa if p["born"] >= hb["inserts"]]
        inserted = ha["inserts"] - hb["inserts"]
        old_evicted = len(before_keys - {p["pgno"] for p in pa})
        hand_b = hb["hand"]
        ahead_b, behind_b = pb[hand_b:] if hand_b < len(pb) else [], pb[:hand_b] if hand_b < len(pb) else pb
        unevictable = [p for p in pb if not p["evictable"]]
        why = collections.Counter(("strong>1" if p["strong"] > 1 else "") + ("pinned" if p["pinned"] else "") + ("dirty" if p["dirty"] else "") + ("locked" if p["locked"] else "") + ("shared" if p["shared"] else "") for p in unevictable)
        hand_a = ha["hand"]
        behind_a = pa[:hand_a] if hand_a < len(pa) else pa
        print(f"pager {k} count {n}: file_reads {ha['file_reads'] - hb['file_reads']} fetches {ha['fetches'] - hb['fetches']} "
              f"inserted {inserted} evicted {ha['evictions'] - hb['evictions']} (older pages {old_evicted}, its own {inserted - len(new)}) kept-own {len(new)}")
        print(f"   before: cap {hb['capacity']} len {hb['len']} hand {hand_b} ahead {len(ahead_b)} refs {refs(ahead_b)} behind {len(behind_b)} refs {refs(behind_b)} "
              f"unevictable {len(unevictable)} {dict(why)} hits(min/med/max) {sorted(p['hits'] for p in pb)[0] if pb else 0}/{sorted(p['hits'] for p in pb)[len(pb)//2] if pb else 0}/{max((p['hits'] for p in pb), default=0)}")
        print(f"   after:  len {ha['len']} hand {hand_a} behind-hand {len(behind_a)} refs {refs(behind_a)} (old {sum(1 for p in behind_a if p['born'] < hb['inserts'])}) "
              f"ahead {len(pa) - len(behind_a)} (own {sum(1 for p in pa[len(behind_a):] if p['born'] >= hb['inserts'])})")
