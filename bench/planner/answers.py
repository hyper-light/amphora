#!/usr/bin/env python3
"""The planner bench's whole-answer check (kiln plan 01a188991617756c84d73f0e9e100330, step 1 (f)):
every row of every query's answer, as `amphora-bench --planner --answers <dir>` writes them, against
the other engines' answers and, given a second directory, against the parent build's.

usage: python3 -I bench/planner/answers.py <answers dir> [<parent's answers dir>]

A directory is what one pass wrote (bench/src/planner/answers.rs): <benchmark>/<query>/query.sql,
the query, and beside it <engine>.tsv, a line a row in the order the engine returned them, values
between tabs in PostgreSQL's COPY text format (\\N is NULL; \\\\, \\t, \\n and \\r are a backslash,
tab, newline and carriage return). A file whose line begins \\! holds why there is no answer: the
run timed out or failed, or the answer passed the writer's bounds. That engine is listed with why
and left out; summary.py lists the runs that didn't finish.

A query's engines are put in groups whose answers agree, and each group but the largest (the first
of equals) is printed against it with the first difference. Against the parent, each engine's answer
is compared with its own there. The last line is "answers.py: N queries compared, M differ": N the
queries with two answers or more to compare, M those where a comparison found a difference. It exits
1 if M is not 0, and 2 if a directory or file isn't as answers.rs writes them.

Order. A query's answers are compared row by row when its outermost statement has ORDER BY, and as
multisets otherwise. query.sql decides it: with comments and string literals set aside, the last
ORDER BY outside every parenthesis is the outermost statement's (one in a subquery, a CTE or a window
is inside parentheses). Each of its keys is matched to an answer column: a number to the column at
that position, a name to the output column it aliases (AS revenue) or names (l_returnflag), any other
key to the output expression written alike. Matching stops at the first key that is none of these,
which is printed, and only the keys before it order the comparison.

Ties. Rows equal on the ordering keys may come in any order. Each answer is cut wherever a row
doesn't tie the row before it, ties being equal integers, text or NULLs, and REALs within the
tolerance; a REAL key within the tolerance ties two rows whatever keys follow it, since its rounding,
not the next key, may be what ordered them. Only the cuts both answers share are kept, and the rows
between two of them are compared as multisets. Where the outermost statement ends in LIMIT n and both
answers have n rows, the rows tied with the n-th may be any of the rows tied there, among them some
neither answer shows: a difference in those rows is still one (nothing tells it from a wrong row),
printed with that said when the rows agree on the ordering keys, for the record to explain.

Values. NULL equals only NULL. Values that both read as decimal numerals are numbers: two integers
are equal exactly, anything else within the tolerance. Every other value is text, equal exactly but
for trailing spaces: PostgreSQL pads a CHAR(n) value with spaces to n characters ('Brand#41   ' in
q16) where the others keep dbgen's text, and TPC-H accepts trailing blanks in an answer (TPC-H 3.0.1,
clause 2.1.3.4 e).

The tolerance is 1e-9, relative (math.isclose). TPC-H's decimals are REAL (binary64) in Amphora and
SQLite and DECIMAL(15,2) in DuckDB and PostgreSQL, which sum them exactly. Amphora and SQLite sum
REALs with Kahan-Babuska-Neumaier compensation (crates/engine/vdbe/execute.rs, SQLite's func.c),
whose error for terms of one sign is about 2u of the sum (u = 2^-53) beyond each term's own rounding
(Higham, Accuracy and Stability of Numerical Algorithms, 2nd ed., section 4.3), and a term is a few
products of decimals each within u: a sum or an average is within about 1e-15 of the exact one. The
first rows of the baseline's SF 1 pass (bench/records/2026-10-26-jupiter-planner-tpch-sf1-a.jsonl)
differ by 4e-17 (q01's sum_charge) and 2e-16 (q08's mkt_share). 1e-9 leaves six orders of magnitude
above that, as the oracles do (bench/src/oracle/engines.rs), and is still under one row's share of
a sum of a billion like rows (SF 100's lineitem has 6e8). TPC-H's own validation (clause 2.1.3.5:
sums within $100, ratios and averages within 1%) admits other arithmetic, and would pass a sum short
of a row.

The rows of a multiset are paired by sorting both answers alike, text and integers before REALs: two
rows that share every text and integer and differ only in REALs closer than the tolerance could be
paired crosswise and reported. No TPC-H or JOB answer has such rows.
"""

import collections
import math
import os
import re
import sys

# answers.rs's MAX_ROWS and MAX_BYTES: a file past them isn't one the writer wrote.
MAX_ROWS = 1 << 20
MAX_BYTES = 32 << 20
# The longest query.sql read: TPC-H's and JOB's are under 4 KiB.
MAX_SQL = 1 << 20
REL_TOL = 1e-9
ENGINES = ["amphora", "sqlite", "duckdb", "duckdb-mt", "postgres"]

INTEGER = re.compile(r"-?[0-9]+")
# No two digit runs side by side: a long field of digits that isn't a numeral fails in linear time.
DECIMAL = re.compile(r"-?(?:[0-9]+(?:\.[0-9]*)?|\.[0-9]+)(?:[eE][-+]?[0-9]+)?")
ESCAPE = re.compile(r"\\(.?)", re.S)
ESCAPES = {"\\": "\\", "t": "\t", "n": "\n", "r": "\r"}
TOKEN = re.compile(r"""(?P<skip>\s+|--[^\n]*|/\*.*?(?:\*/|\Z))
    |(?P<string>'(?:[^']|'')*'?)
    |(?P<quoted>"(?:[^"]|"")*"?)
    |(?P<word>[A-Za-z_][A-Za-z_0-9$]*)
    |(?P<number>[0-9]+(?:\.[0-9]*)?(?:[eE][-+]?[0-9]+)?|\.[0-9]+(?:[eE][-+]?[0-9]+)?)
    |(?P<other>.)""", re.S | re.X)
# What ends a SELECT list at the statement's own level.
CLAUSES = ("from", "where", "group", "having", "window", "order", "limit", "offset", "union", "intersect", "except")

Rows = collections.namedtuple("Rows", "values lines")
Order = collections.namedtuple("Order", "keys limit unmatched")


class Malformed(Exception):
    """A directory or file that isn't as answers.rs writes them."""


def unescape(field):
    def one(m):
        c = ESCAPES.get(m.group(1))
        if c is None:
            raise ValueError(f"an escape answers.rs doesn't write: \\{m.group(1)}")
        return c
    return ESCAPE.sub(one, field) if "\\" in field else field


def value(field):
    """A written value as compared: None for NULL, an int or a float for a decimal numeral, else its
    text without trailing spaces."""
    if field == "\\N":
        return None
    text = unescape(field).rstrip(" ")
    if INTEGER.fullmatch(text):
        try:
            return int(text)
        except ValueError:  # past Python's 4,300 digits: compared as text
            return text
    if DECIMAL.fullmatch(text):
        return float(text)
    return text


def parse(data, where):
    """A file's bytes as Rows, or the refusal's words."""
    text = data.decode("utf-8", errors="surrogateescape")
    try:
        if text.startswith("\\!"):
            return unescape(text[2:].split("\n", 1)[0])
        if text and not text.endswith("\n"):
            raise ValueError("its last row has no newline")
        if text.count("\n") > MAX_ROWS:
            raise ValueError(f"{text.count(chr(10))} rows, past the writer's {MAX_ROWS}")
        lines = text.split("\n")[:-1]
        return Rows([tuple(value(f) for f in line.split("\t")) for line in lines], lines)
    except ValueError as e:
        raise Malformed(f"{where}: {e}") from None


def read(path):
    with open(path, "rb") as f:
        data = f.read(MAX_BYTES + 1)
    if len(data) > MAX_BYTES:
        raise Malformed(f"{path}: more than the writer's {MAX_BYTES} bytes")
    return parse(data, path)


def subdirectories(path):
    return sorted(d for d in os.listdir(path) if os.path.isdir(os.path.join(path, d)))


def directory(root):
    """{(benchmark, query): (its text, {engine: Rows or a refusal's words})} of an answers directory."""
    if not os.path.isdir(root):
        raise Malformed(f"{root}: not a directory")
    found = {}
    for bench in subdirectories(root):
        for query in subdirectories(os.path.join(root, bench)):
            here = os.path.join(root, bench, query)
            try:
                with open(os.path.join(here, "query.sql"), encoding="utf-8", errors="surrogateescape") as f:
                    sql = f.read(MAX_SQL + 1)
            except OSError as e:
                raise Malformed(f"{here}: no query.sql ({e.strerror})") from None
            if len(sql) > MAX_SQL:
                raise Malformed(f"{here}: a query.sql longer than {MAX_SQL} characters")
            engines = {name[:-4]: read(os.path.join(here, name)) for name in sorted(os.listdir(here)) if name.endswith(".tsv")}
            found[(bench, query)] = (sql, engines)
    return found


# ---- the outermost statement's ORDER BY ----

def tokens(sql):
    """sql's tokens with their depth in parentheses, comments and spaces left out."""
    out, depth = [], 0
    for m in TOKEN.finditer(sql):
        if m.lastgroup == "skip":
            continue
        if m.group() == ")":
            depth = max(depth - 1, 0)
        out.append((m.lastgroup, m.group(), depth))
        if m.group() == "(":
            depth += 1
    return out


def word(token, *words):
    return token[0] == "word" and token[1].lower() in words


def count(token):
    """A whole number token's value (a LIMIT, an ORDER BY position), or None; one too long to be
    either is None, not past int()'s digit limit."""
    return int(token[1]) if token[0] == "number" and token[1].isdigit() and len(token[1]) <= 18 else None


def name(token):
    text = token[1][1:-1].replace('""', '"') if token[0] == "quoted" else token[1]
    return text.casefold()


def text(tokens):
    return " ".join(t[1].casefold() for t in tokens)


def split(toks, start, end):
    """toks[start:end] split at the commas of the statement's own level."""
    parts, part = [], []
    for t in toks[start:end]:
        if t[2] == 0 and t[1] == ",":
            parts.append(part)
            part = []
        else:
            part.append(t)
    if part:
        parts.append(part)
    return parts


def select_items(toks):
    """The outermost SELECT's columns: (alias, the column it names, its expression's text) each."""
    start = next((i + 1 for i, t in enumerate(toks) if t[2] == 0 and word(t, "select")), None)
    if start is None:
        return []
    if start < len(toks) and word(toks[start], "distinct", "all"):
        start += 1
    end = next((i for i in range(start, len(toks)) if toks[i][2] == 0 and (word(toks[i], *CLAUSES) or toks[i][1] == ";")), len(toks))
    items = []
    for item in split(toks, start, end):
        if len(item) >= 3 and word(item[-2], "as"):
            items.append((name(item[-1]), None, text(item[:-2])))
        elif len(item) % 2 == 1 and all(t[0] in ("word", "quoted") if i % 2 == 0 else t[1] == "." for i, t in enumerate(item)):
            items.append((None, name(item[-1]), text(item)))
        else:
            items.append((None, None, text(item)))
    return items


def column_of(key, items):
    """The answer column an ORDER BY key orders by, or None."""
    if len(key) >= 2 and word(key[-2], "nulls") and word(key[-1], "first", "last"):
        key = key[:-2]
    if key and word(key[-1], "asc", "desc"):
        key = key[:-1]
    if len(key) >= 2 and word(key[-2], "collate"):
        key = key[:-2]
    if len(key) == 1 and key[0][0] == "number":
        n = count(key[0])
        return n - 1 if n is not None and 1 <= n <= len(items) else None
    if len(key) == 1 and key[0][0] in ("word", "quoted"):
        # An output column's alias first, as SQL resolves the name, then the column one names.
        for field in (0, 1):
            hits = [i for i, item in enumerate(items) if item[field] == name(key[0])]
            if hits:
                return hits[0] if len(hits) == 1 else None
    hits = [i for i, item in enumerate(items) if item[2] == text(key)]
    return hits[0] if len(hits) == 1 else None


def order_of(sql):
    """The outermost statement's ORDER BY keys as answer columns, up to the first unmatched one
    (whose text is kept); its LIMIT n when that ends the statement."""
    toks = tokens(sql)
    top = [t for t in toks if t[2] == 0]
    while top and top[-1][1] == ";":
        top.pop()
    limit = count(top[-1]) if len(top) >= 2 and word(top[-2], "limit") else None
    orders = [i + 2 for i in range(len(toks) - 1) if toks[i][2] == 0 and word(toks[i], "order") and word(toks[i + 1], "by")]
    if not orders:
        return Order([], limit, None)
    end = next((i for i in range(orders[-1], len(toks)) if toks[i][2] == 0 and (word(toks[i], "limit", "offset", "fetch") or toks[i][1] == ";")), len(toks))
    items = select_items(toks)
    keys = []
    for key in split(toks, orders[-1], end):
        column = column_of(key, items)
        if column is None:
            return Order(keys, limit, " ".join(t[1] for t in key))
        keys.append(column)
    return Order(keys, limit, None)


# ---- comparing two answers ----

def as_float(x):
    try:
        return float(x)
    except OverflowError:  # an integer past every double
        return math.copysign(math.inf, x)


def same(u, v):
    """Whether two values agree: NULL only with NULL, numbers as numbers, text exactly."""
    if u is None or v is None:
        return u is None and v is None
    if isinstance(u, str) or isinstance(v, str):
        return u == v
    if isinstance(u, int) and isinstance(v, int):
        return u == v
    return math.isclose(as_float(u), as_float(v), rel_tol=REL_TOL)


def real_columns(*answers):
    """The columns holding a REAL in any row of the answers given."""
    return {i for rows in answers for row in rows for i, v in enumerate(row) if isinstance(v, float)}


def tied(r, s, keys, real):
    for k in keys:
        if not same(r[k], s[k]):
            return False
        if k in real and r[k] is not None:
            return True  # its rounding may have ordered the rows, not the keys after it
    return True


def cuts(rows, keys, real):
    """Where a row doesn't tie the row before it."""
    return {i for i in range(1, len(rows)) if not tied(rows[i - 1], rows[i], keys, real)}


def value_key(v):
    """NULL, then numbers, then text: a value's place in a sort, never comparing a number with text."""
    return (0, 0) if v is None else (2, v) if isinstance(v, str) else (1, v)


def sort_key(row, real):
    """Text, integers and NULLs before REALs, so rows within the tolerance of each other sort alike."""
    return tuple(value_key(v) for i, v in enumerate(row) if i not in real) + tuple(value_key(v) for i, v in enumerate(row) if i in real)


def show(line):
    line = line.replace("\t", " | ")
    return repr(line if len(line) <= 300 else line[:300] + "...")


def rows_same(x, y):
    return len(x) == len(y) and all(same(u, v) for u, v in zip(x, y))


def unmatched(a, ia, b, ib, names, columns=None):
    """The first row of a's ia without an equal among b's ib and the first of b's without one among
    a's, as words, or None where every row pairs; with columns, rows compared on those alone."""
    pick = (lambda row: row) if columns is None else (lambda row: tuple(row[k] for k in columns))
    xs = [pick(a.values[i]) for i in ia]
    ys = [pick(b.values[j]) for j in ib]
    real = real_columns(xs, ys)
    xo = sorted(range(len(xs)), key=lambda i: sort_key(xs[i], real))
    yo = sorted(range(len(ys)), key=lambda j: sort_key(ys[j], real))
    only, i, j = [None, None], 0, 0
    while i < len(xo) and j < len(yo) and None in only:
        x, y = xs[xo[i]], ys[yo[j]]
        if rows_same(x, y):
            i, j = i + 1, j + 1
        elif sort_key(x, real) < sort_key(y, real):
            only[0] = xo[i] if only[0] is None else only[0]
            i += 1
        else:
            only[1] = yo[j] if only[1] is None else only[1]
            j += 1
    if only[0] is None and i < len(xo):
        only[0] = xo[i]
    if only[1] is None and j < len(yo):
        only[1] = yo[j]
    found = [f"only in {names[0]}: {show(a.lines[ia[only[0]]])}"] if only[0] is not None else []
    found += [f"only in {names[1]}: {show(b.lines[ib[only[1]]])}"] if only[1] is not None else []
    return "; ".join(found) or None


def compare(a, b, order, names):
    """How b differs from a, first, in words naming the rows; None if they agree."""
    if len(a.values) != len(b.values):
        return f"{len(a.values)} rows against {len(b.values)}; " + (unmatched(a, range(len(a.values)), b, range(len(b.values)), names) or "")
    widths = {len(r) for r in a.values} | {len(r) for r in b.values}
    if len(widths) > 1:
        return "rows of " + " and ".join(str(w) for w in sorted(widths)) + " values"
    real = real_columns(a.values, b.values)
    edges = [0, *sorted(cuts(a.values, order.keys, real) & cuts(b.values, order.keys, real)), len(a.values)]
    for start, end in zip(edges, edges[1:]):
        if end - start == 1 and rows_same(a.values[start], b.values[start]):
            continue
        found = unmatched(a, range(start, end), b, range(start, end), names)
        if found is None:
            continue
        if order.keys and end == len(a.values) == order.limit and unmatched(a, range(start, end), b, range(start, end), names, order.keys) is None:
            found += f" (they agree on the ORDER BY's keys at LIMIT {order.limit}, where an engine may return any of the rows tied there)"
        return (f"row {start + 1}" if end - start == 1 else f"rows {start + 1} to {end} in any order") + f": {found}"
    return None


def engine_order(engines):
    return sorted(engines, key=lambda e: (ENGINES.index(e) if e in ENGINES else len(ENGINES), e))


def between_engines(rows, order):
    """The engines in groups whose answers agree; each but the largest against it."""
    groups = []
    for engine in engine_order(rows):
        group = next((g for g in groups if compare(rows[g[0]], rows[engine], order, (g[0], engine)) is None), None)
        if group is None:
            groups.append([engine])
        else:
            group.append(engine)
    largest = max(groups, key=len, default=None)
    return [f"{', '.join(g)} against {', '.join(largest)}: {compare(rows[largest[0]], rows[g[0]], order, (largest[0], g[0]))}" for g in groups if g is not largest]


def query_order(key):
    bench, query = key
    digits = "".join(c for c in query if c in "0123456789")
    return (bench, int(digits) if digits else 0, query)


def main(argv):
    # Bytes that aren't UTF-8 (in a value, an engine's words, a file's name) print escaped.
    sys.stdout.reconfigure(errors="backslashreplace")
    sys.stderr.reconfigure(errors="backslashreplace")
    if len(argv) not in (1, 2):
        print(__doc__.split("\n\n")[1], file=sys.stderr)
        return 2
    try:
        build = directory(argv[0])
        parent = directory(argv[1]) if len(argv) == 2 else None
    except (Malformed, OSError) as e:
        print(f"answers.py: {e}", file=sys.stderr)
        return 2
    compared = differ = 0
    for key in sorted(build, key=query_order):
        sql, engines = build[key]
        where = f"{key[0]}/{key[1]}"
        order = order_of(sql)
        if order.unmatched is not None:
            print(f"answers.py: {where}: ORDER BY's {order.unmatched!r} is not an output column: rows equal on the keys before it compared in any order")
        sides = [(engines, "")]
        if parent is not None and key in parent:
            if parent[key][0] != sql:
                print(f"answers.py: {where}: the parent's query.sql differs")
            sides.append((parent[key][1], " in the parent"))
        for answers, side in sides:
            for engine in engine_order(answers):
                if isinstance(answers[engine], str):
                    print(f"answers.py: {where} {engine}{side}: no answer: {answers[engine]}")
        rows = {e: r for e, r in engines.items() if isinstance(r, Rows)}
        differences = between_engines(rows, order)
        pairs = len(rows) >= 2
        if parent is not None and key in parent:
            for engine in engine_order(rows):
                theirs = parent[key][1].get(engine)
                if isinstance(theirs, Rows):
                    pairs = True
                    found = compare(theirs, rows[engine], order, (f"{engine} in the parent", engine))
                    if found:
                        differences.append(f"{engine} against the parent: {found}")
        compared += pairs
        differ += bool(differences)
        for d in differences:
            print(f"answers.py: {where}: {d}")
    print(f"answers.py: {compared} queries compared, {differ} differ")
    return 1 if differ else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
