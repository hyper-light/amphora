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


def main(path):
    literals = open(path).read().split("\n")
    con = sqlite3.connect(":memory:")
    counts = {"literal": 0, "cast": 0, "zero_sign": 0}
    examples = []
    for lit in literals:
        want = float(lit)
        for kind, sql, args in (("literal", f"SELECT {lit}", ()), ("cast", "SELECT CAST(? AS REAL)", (lit,))):
            got = con.execute(sql, args).fetchone()[0]
            if bits(got) == bits(want):
                continue
            if float(got) == 0.0 and want == 0.0:
                counts["zero_sign"] += 1
                continue
            counts[kind] += 1
            if len(examples) < 10:
                examples.append(f"{kind} {lit[:80]}: sqlite {float(got)!r} correctly rounded {want!r}")
    print(f"sqlite {sqlite3.sqlite_version}: {len(literals)} literals; wrong as a literal {counts['literal']}, "
          f"as CAST text {counts['cast']}; zero sign only {counts['zero_sign']}")
    for line in examples:
        print(" ", line)
    return 1 if counts["literal"] or counts["cast"] else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
