#!/usr/bin/env python3
"""The commit bench's verdict (goal "Many clients write through SQL at once, safely": durable commits
benchmarked with 32 writers against SQLite and PostgreSQL; T-T1.4.1). Reads the records
bench/commits.sh wrote (<dir>/records.jsonl: one JSON line per system and writer count, as
bench/commits_fit.py measure prints them) and prints:

  platform: ...; client: ...          the one platform and one driver every record names
  writers <n>: <system> <commits/s> (p50/p99/p99.9 us, cores busy) ...   a line per count of the ladder
  vs <n>: <amphora system>: <x>x SQLite's, <y>x PostgreSQL's commits a second   at each count
  AC-T1.3.2 ... AC-T1.3.5: MEETS|MISSES|UNSETTLED <amphora system> ...   the bars, at 32 writers

The bars are docs/requirements.toml's, format values from there, not chosen here: AC-T1.3.2, at least
20,000 durable single-row commits a second from 32 writers; AC-T1.3.3, p99 at most 5 ms at that load;
AC-T1.3.4, p99.9 at most 15 ms; AC-T1.3.5, 32 writers' throughput at least 10 times one writer's.
Each is judged for each Amphora storage (pager, versioned) that has a record at the counts it needs;
UNSETTLED when one is missing. The requirements measure them on a quiet adacachy NVMe: the platform
line says where these records come from.

Records that don't name one and the same platform and driver aren't a comparison but experiment
error (the brief's "Fair comparisons"): refused with exit 2 before anything is compared. A system
missing at 32 writers (SQLite's or PostgreSQL's record) leaves its comparison UNSETTLED.
Exits 0 when the records were compared (a missed bar is a result, not a failure), 2 when refused.

usage: commits_verdict.py <records.jsonl>
"""
import json
import sys

AT = 32  # AC-T1.3.2 to AC-T1.3.4's load: 32 writers.
BARS = (
    ("AC-T1.3.2", "commits a second", lambda r, one: r["commits_per_s"] >= 20000, lambda r, one: f"{r['commits_per_s']:.0f}/s, bar at least 20000/s"),
    ("AC-T1.3.3", "p99", lambda r, one: r.get("p99_us") is not None and r.get("p99_us") <= 5000, lambda r, one: f"p99 {fmt_us(r.get('p99_us'))}, bar at most 5 ms"),
    ("AC-T1.3.4", "p99.9", lambda r, one: r.get("p999_us") is not None and r.get("p999_us") <= 15000, lambda r, one: f"p99.9 {fmt_us(r.get('p999_us'))}, bar at most 15 ms"),
    ("AC-T1.3.5", "32 writers over 1", lambda r, one: r["commits_per_s"] >= 10 * one["commits_per_s"],
     lambda r, one: f"{r['commits_per_s'] / one['commits_per_s']:.2f}x one writer's, bar at least 10x" if one["commits_per_s"] > 0 else "one writer's rate is 0"),
)
AMPHORA = ("pager", "versioned")
OTHERS = (("sqlite", "SQLite"), ("postgres", "PostgreSQL"))


def fmt_us(us):
    return "none" if us is None else f"{us / 1000:.2f} ms"


def system(r):
    """The record's system: commits_fit.py names it; older records by their engine and storage."""
    if r.get("system"):
        return r["system"]
    eng = r.get("engine") or {}
    return eng.get("storage") if eng.get("engine") == "amphora" else eng.get("engine")


def driver(r):
    c = r.get("client")
    return c.get("driver") if isinstance(c, dict) else c


def read(path):
    with open(path, encoding="utf-8") as f:
        return [json.loads(line) for line in f if line.strip()]


def main(argv):
    if len(argv) != 1:
        sys.exit(__doc__)
    records = read(argv[0])
    if not records:
        print("refused: no records")
        return 2
    where = {json.dumps(r.get("platform"), sort_keys=True) for r in records}
    who = {driver(r) for r in records}
    if len(where) != 1 or len(who) != 1:
        print(f"refused: the records name {len(where)} platforms and {len(who)} drivers, not one of each:"
              f" {sorted(where)} {sorted(map(str, who))}: not a comparison")
        return 2
    print(f"platform: {where.pop()}; client: {who.pop()}")
    by = {}
    for r in records:
        by[(r.get("writers"), system(r))] = r
    for n in sorted({w for w, _ in by}):
        cells = []
        for (w, s), r in sorted(by.items(), key=lambda kv: str(kv[0][1])):
            if w != n:
                continue
            c = r.get("client") or {}
            busy = c.get("cores_busy")
            cells.append(f"{s} {r['commits_per_s']:.0f}/s (p50 {fmt_us(r.get('p50_us'))}, p99 {fmt_us(r.get('p99_us'))},"
                         f" p99.9 {fmt_us(r.get('p999_us'))}, refused {r.get('refused')},"
                         f" cores busy {'?' if busy is None else f'{busy:.2f}'} of {c.get('cores', '?')})")
        print(f"writers {n}: " + "; ".join(cells))
        for a in AMPHORA:
            mine = by.get((n, a))
            if mine is None:
                continue
            vs = []
            for o, name in OTHERS:
                theirs = by.get((n, o))
                if theirs is None:
                    vs.append(f"{name}'s UNSETTLED (no record)")
                elif theirs["commits_per_s"] > 0:
                    vs.append(f"{mine['commits_per_s'] / theirs['commits_per_s']:.2f}x {name}'s")
                else:
                    vs.append(f"{name}'s rate is 0")
            print(f"vs {n}: {a}: " + ", ".join(vs) + " commits a second")
    for a in AMPHORA:
        r, one = by.get((AT, a)), by.get((1, a))
        for ac, what, met, said in BARS:
            needs_one = ac == "AC-T1.3.5"
            if r is None or (needs_one and one is None):
                missing = f"{AT} writers" if r is None else "1 writer"
                print(f"{ac} {what}: UNSETTLED {a}: no record at {missing}")
                continue
            print(f"{ac} {what}: {'MEETS' if met(r, one) else 'MISSES'} {a}: {said(r, one)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
