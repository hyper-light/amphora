#!/usr/bin/env python3
"""sqllogictest's records, read and run as SQLite's own runner does them (G2 step 2): the format of
sqlite.org/sqllogictest (its sqllogictest.c and slt_sqlite.c), so the corpus's SQLite answers are
checked against the answers the files carry.

A file is records separated by blank lines:
  statement ok | statement error      then the SQL
  query <types> <sort> [<label>]      then the SQL, ----, and the answer: one value a line, or
                                      "<n> values hashing to <md5>"
  skipif <engine> | onlyif <engine>   before a record: whether this engine runs it
  hash-threshold <n>                  answers of more values than n are compared by their MD5
  halt                                stop reading the file
Types, as slt_sqlite.c prints them: I is sqlite3_column_int (32 bits) as %d; R is
sqlite3_column_double through SQLite's own printf("%.3f") (16 significant digits, then zeros); T is
sqlite3_column_text up to its first NUL, "(empty)" when that's nothing, every byte outside ' '..'~'
an @; NULL is NULL in any column. Sorts: nosort, rowsort (rows by their values' text), valuesort
(every value).

usage: slt.py <file.test> ...   runs each file on SQLite, prints what disagrees and a count
"""
import contextlib
import functools
import hashlib
import math
import os
import re
import signal
import sqlite3
import sys
import tempfile
import time

ENGINE = "sqlite"


class Record:
    __slots__ = ("kind", "expect", "types", "sort", "label", "sql", "values", "hashed", "line", "conditions")

    def __init__(self, kind, line, conditions):
        self.kind, self.line, self.conditions = kind, line, conditions
        self.expect = self.types = self.sort = self.label = None
        self.sql, self.values, self.hashed = "", None, None

    def runs_on(self, engine):
        for cond, eng in self.conditions:
            if cond == "skipif" and eng == engine:
                return False
            if cond == "onlyif" and eng != engine:
                return False
        return True


def parse(path):
    """Yields ("record", Record), ("hash-threshold", n) and ("halt", conditions) in file order."""
    with open(path, encoding="utf-8", errors="replace") as f:
        lines = f.read().split("\n")
    i, n = 0, len(lines)
    conditions = []
    while i < n:
        line = lines[i].rstrip("\r")
        if not line.strip() or line.startswith("#"):
            i += 1
            continue
        words = line.split()
        head = words[0]
        if head in ("skipif", "onlyif"):
            conditions.append((head, words[1] if len(words) > 1 else ""))
            i += 1
            continue
        if head == "hash-threshold":
            yield ("hash-threshold", int(words[1]))
            i += 1
            continue
        if head == "halt":
            yield ("halt", conditions)
            conditions = []
            i += 1
            continue
        if head in ("statement", "query"):
            rec = Record(head, i + 1, conditions)
            conditions = []
            if head == "statement":
                rec.expect = words[1] if len(words) > 1 else "ok"
            else:
                rec.types = words[1] if len(words) > 1 else ""
                rec.sort = words[2] if len(words) > 2 else "nosort"
                rec.label = words[3] if len(words) > 3 else None
            i += 1
            sql = []
            while i < n and lines[i].strip() and lines[i].rstrip("\r") != "----":
                sql.append(lines[i].rstrip("\r"))
                i += 1
            rec.sql = "\n".join(sql)
            if head == "query" and i < n and lines[i].rstrip("\r") == "----":
                i += 1
                vals = []
                while i < n and lines[i].strip():
                    vals.append(lines[i].rstrip("\r"))
                    i += 1
                m = re.fullmatch(r"(\d+) values hashing to ([0-9a-f]{32})", vals[0]) if len(vals) == 1 else None
                if m:
                    rec.hashed = (int(m.group(1)), m.group(2))
                else:
                    rec.values = vals
            yield ("record", rec)
            continue
        i += 1  # a line this reader doesn't know: skipped, as the C runner does


INT_PREFIX = re.compile(r"\s*([+-]?\d+)")
REAL_PREFIX = re.compile(r"\s*([+-]?(\d+\.?\d*([eE][+-]?\d+)?|\.\d+([eE][+-]?\d+)?))")
INT64_MIN, INT64_MAX = -(1 << 63), (1 << 63) - 1


def _text(v):
    """A text or blob value's characters one per byte, as the C runner sees them."""
    return bytes(v).decode("latin-1") if isinstance(v, (bytes, bytearray)) else v


def as_int(v):
    """sqlite3VdbeIntValue's view of a value: reals truncated (saturating), text by its leading
    integer (saturating)."""
    if isinstance(v, int):
        return v
    if isinstance(v, float):
        if math.isnan(v):
            return 0
        if v >= 9.223372036854775807e18:
            return INT64_MAX
        if v <= -9.223372036854775808e18:
            return INT64_MIN
        return int(v)
    m = INT_PREFIX.match(_text(v))
    return max(INT64_MIN, min(INT64_MAX, int(m.group(1)))) if m else 0


def as_real(v):
    """sqlite3VdbeRealValue's view: text by its longest leading real."""
    if isinstance(v, (int, float)):
        return float(v)
    m = REAL_PREFIX.match(_text(v))
    return float(m.group(1)) if m else 0.0


_SQLITE = [None, None]  # an in-memory connection of this process's own, for SQLite's renderings


def _sqlite():
    if _SQLITE[1] != os.getpid():
        _SQLITE[0], _SQLITE[1] = sqlite3.connect(":memory:"), os.getpid()
    return _SQLITE[0]


@functools.lru_cache(maxsize=1 << 16)
def real3(r):
    """SQLite's printf("%.3f"), as sqlite3_snprintf renders it for the C runner."""
    return _sqlite().execute("SELECT printf('%.3f', ?)", (r,)).fetchone()[0]


@functools.lru_cache(maxsize=1 << 16)
def real_text(r):
    """A REAL as sqlite3_column_text renders it (this SQLite's own digits)."""
    return _sqlite().execute("SELECT CAST(? AS TEXT)", (r,)).fetchone()[0]


def format_value(v, t):
    if v is None:
        return "NULL"
    if t == "I":
        return str((as_int(v) + (1 << 31)) % (1 << 32) - (1 << 31))
    if t == "R":
        return real3(as_real(v))
    if isinstance(v, (bytes, bytearray)):
        raw = bytes(v)
    elif isinstance(v, str):
        raw = v.encode("utf-8")
    elif isinstance(v, float):
        raw = real_text(v).encode()
    else:
        raw = str(v).encode()
    raw = raw.split(b"\0", 1)[0]
    if not raw:
        return "(empty)"
    return "".join(chr(b) if 0x20 <= b <= 0x7E else "@" for b in raw)


def column_types(rows, ncol):
    """Types for an answer written from SQLite's own rows: I for integers that print whole in 32
    bits, R for numbers %.3f prints without loss of scale, T (the exact text) otherwise."""
    out = ""
    for j in range(ncol):
        vals = [r[j] for r in rows if r[j] is not None]
        if vals and all(isinstance(v, int) and -(1 << 31) <= v < (1 << 31) for v in vals):
            out += "I"
        elif vals and all(isinstance(v, (int, float)) and (v == 0 or 1e-3 <= abs(v) < 1e12) for v in vals):
            out += "R"
        else:
            out += "T"
    return out or "T"


def regexp(pattern, value):
    """REGEXP as probe.py's oracle gives it (Python's re.search): SQLite itself has no regexp()."""
    if pattern is None or value is None:
        return None
    pattern, value = (_text(x) if isinstance(x, (bytes, bytearray)) else str(x) for x in (pattern, value))
    return int(re.search(pattern, value) is not None)


LIMIT = 20  # seconds a record may take, translation and SQLite together: a pathological one is cut
ROWS = 1_000_000  # rows a statement's answer may have before it's cut the same way (a runaway one
# fills memory faster than its time runs out: one OOM-killed a worker on 2026-10-24)
_DEADLINE = [None, False]  # the record's deadline, and whether it ran out of time or rows


class OverBudget(Exception):
    """A record ran past its time or its rows."""


def fetch(cur):
    """A statement's rows within its record's budget: past ROWS of them, OverBudget."""
    rows = []
    while True:
        chunk = cur.fetchmany(10000)
        if not chunk:
            return rows
        rows.extend(chunk)
        if len(rows) > ROWS:
            _DEADLINE[1] = True
            raise OverBudget()


def _ring(signum, frame):
    _DEADLINE[1] = True
    raise OverBudget()


def _progress():
    if _DEADLINE[0] is not None and time.monotonic() > _DEADLINE[0]:
        _DEADLINE[1] = True
        return 1  # SQLite stops the statement: "interrupted"
    return 0


@contextlib.contextmanager
def budget(seconds=LIMIT):
    """A record's time: an alarm stops Python (sqlglot's parser), the progress handler stops SQLite
    (rolling the statement back). Inside, OverBudget or SQLite's "interrupted" says it ran out;
    `spent()` says so after."""
    _DEADLINE[0], _DEADLINE[1] = time.monotonic() + seconds, False
    old = signal.signal(signal.SIGALRM, _ring)
    signal.setitimer(signal.ITIMER_REAL, seconds)
    try:
        yield
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, old)
        _DEADLINE[0] = None


def spent():
    return _DEADLINE[1]


def connect(db, text=bytes):
    """A connection as the corpus's runs make them: autocommit, no busy wait, synchronous off (as the
    C runner sets it), REGEXP given, and text read as bytes (the C runner reads bytes; TEXT that isn't
    UTF-8 doesn't stop the run)."""
    con = sqlite3.connect(db, isolation_level=None, timeout=0)
    con.text_factory = text
    con.create_function("regexp", 2, regexp, deterministic=True)
    con.execute("PRAGMA synchronous=OFF")  # as slt_sqlite.c opens its test.db: a throwaway file
    con.set_progress_handler(_progress, 100000)  # what keeps a statement to its record's budget
    return con


def answer(rows, types, sort):
    """The query's answer as the C runner writes it: formatted values in their sort's order."""
    ncol = len(types)
    vals = [[format_value(r[j] if j < len(r) else None, types[j] if j < ncol else "T") for j in range(max(ncol, len(r)))] for r in rows]
    if sort == "rowsort":
        vals.sort()
    flat = [v for r in vals for v in r]
    if sort == "valuesort":
        flat.sort()
    return flat


def md5_of(values):
    h = hashlib.md5()
    for v in values:
        h.update(v.encode("utf-8"))
        h.update(b"\n")
    return h.hexdigest()


def split(sql):
    """A record's statements, where SQLite's own parser says each ends (a trigger keeps its body)."""
    out, cur = [], ""
    for piece in sql.split(";"):
        cur = piece if not cur else cur + ";" + piece
        if sqlite3.complete_statement(cur + ";"):
            if cur.strip():
                out.append(cur.strip())
            cur = ""
    if cur.strip():
        out.append(cur.strip())
    return out or [sql]


UNSTABLE = re.compile(r"\b(random|randomblob)\s*\(|\bcurrent_(timestamp|date|time)\b|'now'", re.I)


def stable(sql):
    """Whether a statement answers the same every run: nothing from random() or the clock (such a
    query is kept as a statement, run but its answer not checked)."""
    return not UNSTABLE.search(sql)


RECORD_TOKEN = re.compile(r"""(?P<comment>--[^\n]*|/\*.*?(?:\*/|\Z))
  | (?P<lit>[xX]?'(?:[^']|'')*'?|"(?:[^"]|"")*"?|\[[^\]]*\]?|`[^`]*`?)
  | (?P<ws>\s+)
  | (?P<other>[^\s'"`\[\-/xX]+|.)""", re.S | re.X)
BLANK_LINE = re.compile(r"\n[ \t\r]*\n")


def record_sql(sql):
    """The statement as a record can carry it: a blank line ends a record, so blank lines are taken
    out where they are whitespace or comment (the statement means the same); None when one is inside
    a literal, or a line is ---- (which ends a record's SQL)."""
    out = []
    for m in RECORD_TOKEN.finditer(sql):
        t = m.group()
        if m.lastgroup == "ws" and BLANK_LINE.search(t):
            t = "\n"
        elif m.lastgroup == "comment":
            while BLANK_LINE.search(t):
                t = BLANK_LINE.sub("\n", t)
        elif m.lastgroup == "lit" and BLANK_LINE.search(t):
            return None
        out.append(t)
    text = "".join(out).strip()
    if not text or any(line.rstrip("\r") == "----" for line in text.split("\n")):
        return None
    return text


def run_file(path, keep=None, fixture=None):
    """Runs one file on a fresh SQLite database (after `fixture`'s statements, a file of the base
    tables a suite's files share, when given). Returns counts and, for each record that doesn't
    answer as the file says, (line, kind, sql, what the file says, what SQLite says); `keep` (a
    callable) sees every query run with SQLite's answer."""
    fd, db = tempfile.mkstemp(prefix="slt-", suffix=".db", dir=os.path.expanduser("~/.cache"))
    os.close(fd)
    con = connect(db)
    for kind, item in parse(fixture) if fixture else ():
        if kind == "record" and item.kind == "statement":
            for part in split(item.sql):
                con.execute(part).fetchall()
    counts = {"statements": 0, "queries": 0, "skipped": 0, "agree": 0, "disagree": 0}
    off = []
    threshold = 0
    try:
        for kind, item in parse(path):
            if kind == "hash-threshold":
                threshold = item
                continue
            if kind == "halt":
                if all((c == "skipif" and e != ENGINE) or (c == "onlyif" and e == ENGINE) for c, e in item):
                    break
                continue
            rec = item
            if not rec.runs_on(ENGINE):
                counts["skipped"] += 1
                continue
            if rec.kind == "statement":
                counts["statements"] += 1
                try:
                    with budget():
                        for part in split(rec.sql):
                            fetch(con.execute(part))  # to the end, as sqlite3_exec runs it
                    ok = rec.expect == "ok"
                    got = "ok"
                except (sqlite3.Error, OverBudget) as e:
                    ok = rec.expect == "error" and not spent()
                    got = "over budget" if spent() else f"error: {e}"
                counts["agree" if ok else "disagree"] += 1
                if not ok:
                    off.append((rec.line, "statement", rec.sql, rec.expect, got))
                if keep:
                    keep(rec, got)
                continue
            counts["queries"] += 1
            try:
                with budget():
                    rows = fetch(con.execute(rec.sql))
            except (sqlite3.Error, OverBudget) as e:
                got = "over budget" if spent() else f"error: {e}"
                counts["disagree"] += 1
                off.append((rec.line, "query", rec.sql, "an answer", got))
                if keep:
                    keep(rec, got)
                continue
            vals = answer(rows, rec.types, rec.sort)
            if rec.hashed or (threshold and len(vals) > threshold):
                got = f"{len(vals)} values hashing to {md5_of(vals)}"
                want = f"{rec.hashed[0]} values hashing to {rec.hashed[1]}" if rec.hashed else f"{len(rec.values)} values hashing to {md5_of(rec.values)}"
            else:
                got, want = vals, rec.values
            ok = got == want
            counts["agree" if ok else "disagree"] += 1
            if not ok:
                off.append((rec.line, "query", rec.sql, want if isinstance(want, str) else want[:12], got if isinstance(got, str) else got[:12]))
            if keep:
                keep(rec, vals)
    finally:
        con.close()
        os.remove(db)
    return counts, off


if __name__ == "__main__":
    total = {}
    for p in sys.argv[1:]:
        counts, off = run_file(p)
        for k, v in counts.items():
            total[k] = total.get(k, 0) + v
        for line, kind, sql, want, got in off[:20]:
            print(f"{p}:{line}: {kind}: {sql[:120]!r}\n   file says {want!r}\n   SQLite says {got!r}")
    print(total)
