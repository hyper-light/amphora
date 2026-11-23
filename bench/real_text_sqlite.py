#!/usr/bin/env python3
"""Lane G10: a REAL's text three ways over the engine's corpus: Amphora's CAST(x AS TEXT), SQLite's,
and https://www.sqlite.org/floatingpoint.html 1.3.1's algorithm ("Round to 15 digits. Convert the
text result back to binary-64. If the binary-64 value from the previous step is different, then:
Redo step (a) with 17-digit rounding."), 17 digits as the page's example rounds them (the first 18,
then 5 or more rounds up: "If you round to 17 digits, you get 2.8781999999999997"). Write the corpus
with `AMPHORA_REAL_TEXT_CORPUS=<file> cargo test -p amphora-engine --test floating_point_doc
g10_real_text_corpus`, then `python3 -I bench/real_text_sqlite.py <file>`. Exits 1 if Amphora
differs from the page on any double."""

import sqlite3
import struct
import sys
from decimal import ROUND_HALF_EVEN, ROUND_HALF_UP, Decimal, localcontext


def rounded(x, n, mode):
    with localcontext() as c:
        c.prec = n
        c.rounding = mode
        return +Decimal(x)


def layout(neg, q):
    _, digs, exp = q.as_tuple()
    s = "".join(map(str, digs)).rstrip("0") or "0"
    point = len(digs) + exp
    e = point - 1
    out = "-" if neg else ""
    if e < -4 or e > 16:
        return out + s[0] + "." + (s[1:] or "0") + "e" + ("-" if e < 0 else "+") + "%02d" % abs(e)
    if point <= 0:
        return out + "0." + "0" * (-point) + s
    if point >= len(s):
        return out + s + "0" * (point - len(s)) + ".0"
    return out + s[:point] + "." + s[point:]


def page(x):
    if x == 0:
        return "0.0", "15"
    a = abs(x)
    q = rounded(a, 15, ROUND_HALF_EVEN)
    if float(q) == a:
        return layout(x < 0, q), "15"
    q = rounded(rounded(a, 18, ROUND_HALF_EVEN), 17, ROUND_HALF_UP)
    return layout(x < 0, q), "17"


def main(path):
    con = sqlite3.connect(":memory:")
    counts = {}
    examples = []
    total = 0
    for line in open(path):
        bits, amphora = line.rstrip("\n").split("\t")
        x = struct.unpack("<d", struct.pack("<Q", int(bits, 16)))[0]
        lite = con.execute("SELECT CAST(? AS TEXT)", (x,)).fetchone()[0]
        want, path_ = page(x)
        total += 1
        for name, ok in (("amphora=page", amphora == want), ("sqlite=page", lite == want), ("amphora=sqlite", amphora == lite)):
            c = counts.setdefault((name, path_), [0, 0])
            c[0] += 1
            c[1] += ok
        if amphora != want and len(examples) < 10:
            examples.append(f"{x!r}: amphora {amphora} page {want} sqlite {lite}")
    print(f"sqlite {sqlite3.sqlite_version}: {total} doubles")
    for name in ("amphora=page", "sqlite=page", "amphora=sqlite"):
        for p in ("15", "17"):
            n, ok = counts.get((name, p), [0, 0])
            print(f"  {name} where the page writes {p} digits: {ok} of {n} agree ({100.0 * ok / max(n, 1):.3f}%)")
    for e in examples:
        print("  amphora differs from the page:", e)
    wrong = sum(n - ok for (name, _), (n, ok) in counts.items() if name == "amphora=page")
    return 1 if wrong else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
