#!/usr/bin/env python3
"""kiln's own statements (G2 step 2): kiln's schema as production makes it (its store's migrations, in
order), then every SQL statement kiln's crates write, read from its source at a named revision
(string literals that are statements; a placeholder naming a constant of the same file is filled
from it, one built from anything else is left out and counted), each run on SQLite with its
parameters unbound (SQLite takes them as NULL), all written as one sqllogictest file with SQLite's
answers.

usage: kiln_queries.py <kiln checkout> <revision> <out dir>
  writes <out dir>/kiln/kiln.test and prints counts
"""
import os
import re
import sqlite3
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import slt  # noqa: E402

REPO, REV, OUT = sys.argv[1:4]
STATEMENT = re.compile(r"^\s*(SELECT|INSERT|REPLACE|UPDATE|DELETE|CREATE|DROP|ALTER|WITH|PRAGMA|BEGIN|COMMIT|ROLLBACK|SAVEPOINT|RELEASE|VACUUM|ANALYZE|REINDEX)\b", re.I | re.S)
LITERAL = re.compile(r'r(#+)"(.*?)"\1|r"(.*?)"|"((?:[^"\\]|\\.)*)"', re.S)


def sources():
    files = subprocess.run(["git", "-C", REPO, "ls-tree", "-r", "--name-only", REV], capture_output=True, text=True, check=True).stdout.split("\n")
    for f in files:
        if f.endswith(".rs") and f.startswith("crates/") and "/tests/" not in f and not f.startswith("vendor/"):
            yield f, subprocess.run(["git", "-C", REPO, "show", f"{REV}:{f}"], capture_output=True, text=True).stdout


RUST_ESCAPE = re.compile(r"\\(\n\s*|u\{([0-9a-fA-F]+)\}|x([0-9a-fA-F]{2})|.)", re.S)


def unescape(s):
    """A Rust string literal's escapes: a backslash-newline joins lines without their indentation."""
    def one(m):
        c = m.group(1)
        if c.startswith("\n") or c[0] == "\n":
            return ""
        if m.group(2):
            return chr(int(m.group(2), 16))
        if m.group(3):
            return chr(int(m.group(3), 16))
        return {"n": "\n", "t": "\t", "r": "\r", "0": "\0", "\\": "\\", '"': '"', "'": "'"}.get(c, c)
    return RUST_ESCAPE.sub(one, s)


SQL_SHAPE = re.compile(r"\b(FROM|INTO|TABLE|SET|VALUES|INDEX|VIEW|TRIGGER|PRAGMA|AS)\b", re.I)


def is_sql(sql):
    """A literal that's a statement, not a word that begins like one ("commit", "Create an app"):
    more than one word, a clause a statement has, and SQLite's dialect parses it."""
    import sqlglot
    if len(sql.split()) < 2 or not SQL_SHAPE.search(sql):
        return False
    try:
        return all(e is not None for e in sqlglot.parse(sql, read="sqlite"))
    except Exception:
        return False


CONST = re.compile(r'const\s+([A-Z_][A-Z_0-9]*)\s*:\s*&str\s*=\s*"([^"\\]*)"\s*;')


def migrations():
    """The store's migrations, in the order their names give: kiln's production schema."""
    files = subprocess.run(["git", "-C", REPO, "ls-tree", "-r", "--name-only", REV, "crates/kiln/src/store/migrations/"], capture_output=True, text=True, check=True).stdout.split()
    for f in sorted(x for x in files if x.endswith(".sql")):
        yield f, subprocess.run(["git", "-C", REPO, "show", f"{REV}:{f}"], capture_output=True, text=True).stdout


def statements():
    seen, placeholders = [], 0
    for path, text in sources():
        consts = dict(CONST.findall(text))
        for m in LITERAL.finditer(text):
            raw = m.group(2) if m.group(2) is not None else m.group(3) if m.group(3) is not None else m.group(4)
            if not STATEMENT.match(raw or ""):
                continue
            if m.group(4) is not None:
                raw = unescape(raw)
            if "\x00" in raw:
                continue
            sql = re.sub(r"\{([A-Z_][A-Z_0-9]*)\}", lambda c: consts.get(c.group(1), c.group(0)), raw.strip().rstrip(";").strip())
            for part in slt.split(sql):
                part = part.strip().rstrip(";").strip()
                if not part or not STATEMENT.match(part):
                    continue
                if re.search(r"\{[A-Za-z_0-9:]*\}", part):
                    placeholders += 1
                    continue
                if is_sql(part):
                    seen.append((path, text.count("\n", 0, m.start()) + 1, part))
    return seen, placeholders


def main():
    found, placeholders = statements()
    uniq, seen = [], set()
    for path, line, sql in found:
        key = re.sub(r"\s+", " ", sql)
        if key not in seen:
            seen.add(key)
            uniq.append((path, line, sql))
    schema = [s for s in uniq if re.match(r"\s*CREATE\b", s[2], re.I)]
    rest = [s for s in uniq if not re.match(r"\s*CREATE\b", s[2], re.I)]
    fd, db = tempfile.mkstemp(prefix="kiln-", suffix=".db", dir=os.path.expanduser("~/.cache"))
    os.close(fd)
    con = slt.connect(db)
    lines = [f"# kiln's statements at {REV}, from its crates' source: schema first, then each statement", "hash-threshold 8", ""]
    counts = {"migration_statements": 0, "literals": len(found), "distinct": len(uniq), "placeholders": placeholders, "kept": 0, "refused": 0}
    refused = []
    # The schema: the tables kiln's code makes, then its migrations, in as many passes as it takes
    # (a migration alters a table the code makes; a table can name one made later).
    pending = [(p_, l_, s_, "code") for p_, l_, s_ in schema]
    for path, text in migrations():
        for part in slt.split(text):
            part = part.strip().rstrip(";").strip()
            if part and re.search(r"[A-Za-z]", re.sub(r"--[^\n]*", "", part)):
                pending.append((path, 0, part, "migration"))
    for _ in range(6):
        left = []
        for path, line, sql, origin in pending:
            if slt.record_sql(sql) is None:  # a blank line inside a literal: no record carries it
                counts["unrepresentable"] = counts.get("unrepresentable", 0) + 1
                continue
            try:
                con.execute(sql)
            except sqlite3.Error as e:
                if "already exists" in str(e) or "duplicate column" in str(e):
                    counts["already_made"] = counts.get("already_made", 0) + 1
                    continue
                left.append((path, line, sql, origin, str(e)))
                continue
            counts["migration_statements" if origin == "migration" else "kept"] += 1
            lines += [f"# {path}" + (f":{line}" if line else ""), "statement ok", slt.record_sql(sql), ""]
        if len(left) == len(pending) or not left:
            pending = left
            break
        pending = [(p_, l_, s_, o_) for p_, l_, s_, o_, _ in left]
    for p_, l_, s_, o_, e in pending if pending and len(pending[0]) == 5 else []:
        counts["refused"] += 1
        refused.append((p_, l_, s_[:160], e))
    for path, line, sql in rest:
        if re.match(r"\s*PRAGMA\s+query_only\b", sql, re.I):
            # A read-only connection's setting: kept out, or every write after it would fail.
            counts["connection_settings_left_out"] = counts.get("connection_settings_left_out", 0) + 1
            continue
        if slt.record_sql(sql) is None:
            counts["unrepresentable"] = counts.get("unrepresentable", 0) + 1
            continue
        try:
            cur = con.execute(sql)
            rows = cur.fetchall() if cur.description else None
        except sqlite3.Error as e:
            counts["refused"] += 1
            refused.append((path, line, sql[:160], str(e)))
            continue
        counts["kept"] += 1
        sql = slt.record_sql(sql)
        if rows is None or not slt.stable(sql):  # an answer from random() or the clock isn't kept
            lines += [f"# {path}:{line}", "statement ok", sql, ""]
        else:
            types = slt.column_types(rows, len(cur.description))
            vals = slt.answer(rows, types, "rowsort")
            body = [f"{len(vals)} values hashing to {slt.md5_of(vals)}"] if len(vals) > 8 or any(not v.strip() for v in vals) else vals
            lines += [f"# {path}:{line}", f"query {types} rowsort", sql, "----"] + body + [""]
    con.close()
    os.remove(db)
    os.makedirs(os.path.join(OUT, "kiln"), exist_ok=True)
    open(os.path.join(OUT, "kiln", "kiln.test"), "w").write("\n".join(lines))
    print(counts)
    for r in refused[:40]:
        print("refused:", r)


main()
