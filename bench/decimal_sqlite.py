#!/usr/bin/env python3
"""Lane G9: SQLite's double for each literal of the engine's decimal corpus, against the correctly
rounded double (Python's float, as Rust's parse). Write the corpus with
`AMPHORA_DECIMAL_CORPUS=<file> cargo test -p amphora-engine --lib g9_decimal_text`, then
`python3 -I bench/decimal_sqlite.py <file>`. Exits 1 if SQLite differs on any literal."""

import sqlite3
import struct
import sys


def bits(x):
    return struct.unpack("<Q", struct.pack("<d", float(x)))[0]


def significant(lit):
    mantissa = lit.lstrip("+-").split("e")[0].split("E")[0].replace(".", "")
    return len(mantissa.lstrip("0").rstrip("0"))


def bucket(lit):
    n = significant(lit)
    return "<=17" if n <= 17 else "18-19" if n <= 19 else ">=20"


def main(path):
    literals = open(path).read().split("\n")
    con = sqlite3.connect(":memory:")
    counts = {"literal": 0, "cast": 0, "zero_sign": 0}
    by_digits = {}
    examples = []
    for lit in literals:
        want = float(lit)
        seen = by_digits.setdefault(bucket(lit), [0, 0])
        seen[0] += 1
        for kind, sql, args in (("literal", f"SELECT {lit}", ()), ("cast", "SELECT CAST(? AS REAL)", (lit,))):
            got = con.execute(sql, args).fetchone()[0]
            if bits(got) == bits(want):
                continue
            if float(got) == 0.0 and want == 0.0:
                counts["zero_sign"] += 1
                continue
            counts[kind] += 1
            if kind == "literal":
                seen[1] += 1
            if len(examples) < 10 and kind == "literal":
                examples.append(f"{lit[:40]}..({len(lit)} chars): sqlite {float(got)!r} correctly rounded {want!r}")
    print(f"sqlite {sqlite3.sqlite_version}: {len(literals)} literals; wrong as a literal {counts['literal']}, "
          f"as CAST text {counts['cast']}; zero sign only {counts['zero_sign']}")
    for name in ("<=17", "18-19", ">=20"):
        total, wrong = by_digits.get(name, [0, 0])
        print(f"  {name} significant digits: {wrong} of {total} wrong")
    for line in examples:
        print(" ", line)
    return 1 if counts["literal"] or counts["cast"] else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
