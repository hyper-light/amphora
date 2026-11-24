"""Lane R10: SQLite's own answers (the sqlite3 library Python links, printed first) to REAL
filters with bound parameters and SUM over an INTEGER column holding stray REALs, as Rust cases
for columns_sql's r10 test: each cell `i<int>`, `r<f64 bits hex>`, `n`, `t<text>`."""
import sqlite3
import struct

ROWS = """INSERT INTO r SELECT value, value % 10, (value % 50) * 0.5 - 5.0 FROM generate_series(1, 600);
INSERT INTO r VALUES (601, 7, -0.0), (602, 7, 1.7976931348623157e308), (603, 7, -1.7976931348623157e308),
 (604, 7, 4.9406564584124654e-324), (605, 7, 9007199254740993), (606, 7, 9223372036854775807),
 (607, 2.5, 1.0), (608, 1.7976931348623157e308, 2.0), (609, 1.7976931348623157e308, 3.0),
 (610, NULL, NULL), (611, -9223372036854775808, -9223372036854775808), (612, 7, 'abc'), (613, -0.0, 4.0)"""

ONE = [500, 500.0, 0, -0.0, 0.0, -5, 1.7976931348623157e308, -1.7976931348623157e308,
       9007199254740992, 9007199254740993, 9223372036854775807, -9223372036854775808,
       "500", "1e400", "-0.0", "abc", None, b"\x01", 5e-324]
QUERIES = [
    ("SELECT COUNT(*) FROM r WHERE p < ?", [(v,) for v in ONE]),
    ("SELECT COUNT(*) FROM r WHERE p >= ?", [(v,) for v in ONE]),
    ("SELECT COUNT(*) FROM r WHERE p = ?", [(v,) for v in ONE]),
    ("SELECT COUNT(*) FROM r WHERE ? <> p", [(v,) for v in ONE]),
    ("SELECT COUNT(*) FROM r WHERE q = ? AND p < ?", [(7, 500.0), (7, 0), (2.5, 2), (7, "1e400"), (0, -0.0)]),
    ("SELECT SUM(q), COUNT(q), MIN(q), MAX(q) FROM r WHERE p > ?", [(-2.0,), (0.5,), (1.5,), (2.5,), (3.0,), (None,)]),
    ("SELECT SUM(q), COUNT(*) FROM r WHERE p < ?", [(1.0,), (1.5,), (-4.0,)]),
]


def bits(v):
    return struct.unpack("<Q", struct.pack("<d", v))[0]


def cell(v):
    if v is None:
        return "n"
    if isinstance(v, int):
        return f"i{v}"
    if isinstance(v, float):
        return f"r{bits(v):016x}"
    if isinstance(v, str):
        return "t" + v
    return "b" + v.hex()


def rust(v):
    if v is None:
        return "Value::Null"
    if isinstance(v, int):
        return f"Value::from_i64({v})" if v != -9223372036854775808 else "Value::from_i64(i64::MIN)"
    if isinstance(v, float):
        return f"Value::from_f64(f64::from_bits(0x{bits(v):016x}))"
    if isinstance(v, str):
        return f'Value::build_text("{v}")'
    return f"Value::from_blob(vec![{', '.join(str(b) for b in v)}])"


db = sqlite3.connect(":memory:")
db.execute("CREATE TABLE r (id INTEGER PRIMARY KEY, q INTEGER, p REAL)")
first, rest = ROWS.split(";\n", 1)
assert "generate_series(1, 600)" in first
for v in range(1, 601):
    db.execute("INSERT INTO r VALUES (?1, ?1 % 10, (?1 % 50) * 0.5 - 5.0)", (v,))
db.executescript(rest)
print(f"// SQLite {sqlite3.sqlite_version}")
for at, (sql, cases) in enumerate(QUERIES):
    for params in cases:
        out = db.execute(sql, params).fetchall()
        got = ";".join(",".join(cell(v) for v in row) for row in out)
        print(f'({at}, vec![{", ".join(rust(v) for v in params)}], "{got}"),')
