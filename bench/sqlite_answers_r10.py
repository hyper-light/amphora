"""Lane R10: SQLite's own answers (the sqlite3 library Python links, printed first) to REAL
filters with bound parameters and SUM over an INTEGER column holding stray REALs, as Rust cases
for columns_sql's r10 test: each cell `i<int>`, `r<f64 bits hex>`, `n`, `t<text>`.

Lane R11: then, after `// R11`, its answers to S8's aggregates_range and other searches by a rowid
range between parameters on S8's `items` columns, with rows at both ends of the rowids, as Rust
cases for columns_sql's r11 test: each bound an integer, a real, a numeric text, another text, a
blob and NULL, at and past both ends, empty and reversed."""
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


MIN, MAX = -9223372036854775808, 9223372036854775807
ITEMS_EDGES = [(MIN, -0.0, 4503599627370496, -5), (MAX, 0.1, 7, 4611686018427387904)]
RANGES = [(10, 1009), (1, 3000), (0, 3001), (MIN, MAX), (3000, 3000), (1, 1), (1009, 10),
          (5000, 6000), (MAX, MAX), (MIN, MIN), (9.5, 20.5), (-0.5, 2.0), (2.0, 2.0), (2.5, 2.7),
          (1e300, 2e300), (-1e300, 1e300), (-0.0, 3.0), (9.223372036854775807e18, 1e19),
          (-1e19, -9.223372036854775807e18), ("10", "1009"), (" 10 ", "20"), ("9.5", "20.5"),
          ("1e1", "2e1"), ("1e400", 5), ("-1e400", "3"), ("abc", 20), (10, "abc"), (b"\x01", 20),
          (10, b"\x01"), (None, 20), (10, None), (5, 2.5), ("12abc", 20)]
ONE_BOUND = [2999, 2999.5, 3000, 0.5, -0.0, MIN, MAX, 1e300, -1e300, "2999", "abc", b"\x00", None]
RANGE_QUERIES = [
    ("SELECT SUM(price), AVG(qty), MIN(created), MAX(created) FROM items WHERE id BETWEEN ? AND ?", RANGES),
    ("SELECT AVG(qty), MIN(created), MAX(created), COUNT(*) FROM items WHERE id BETWEEN ? AND ?", RANGES),
    ("SELECT COUNT(*), SUM(qty) FROM items WHERE id >= ? AND id < ?",
     [(10, 20), (10, 10), (9.5, 10.5), (MIN, MIN), (2999, MAX), (MIN, -4), ("10", 20.5), (10, "abc")]),
    ("SELECT COUNT(*), AVG(qty) FROM items WHERE id > ?", [(v,) for v in ONE_BOUND]),
    ("SELECT COUNT(*), MAX(created) FROM items WHERE id <= ?", [(v,) for v in ONE_BOUND]),
]

db.execute("CREATE TABLE items (id INTEGER PRIMARY KEY, price REAL NOT NULL, qty INTEGER, created INTEGER NOT NULL)")
for v in range(1, 3001):
    db.execute("INSERT INTO items VALUES (?, ?, ?, ?)", (v, (v % 1000) * 1.25, None if v % 97 == 0 else v % 100, 1700000000 + v))
db.executemany("INSERT INTO items VALUES (?, ?, ?, ?)", ITEMS_EDGES)
print("// R11")
for at, (sql, cases) in enumerate(RANGE_QUERIES):
    plan = db.execute("EXPLAIN QUERY PLAN " + sql, cases[0]).fetchall()
    assert any("INTEGER PRIMARY KEY" in row[-1] for row in plan), (sql, plan)
    for params in cases:
        out = db.execute(sql, params).fetchall()
        got = ";".join(",".join(cell(v) for v in row) for row in out)
        print(f'({at}, vec![{", ".join(rust(v) for v in params)}], "{got}"),')
