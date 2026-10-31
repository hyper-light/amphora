#!/usr/bin/env python3
"""Other engines' test suites in SQLite's dialect (G2 step 2): each file's statements and queries
translated by sqlglot, run in order on a fresh SQLite database, and kept where SQLite runs them, with
SQLite's answers as the corpus's own (written out as sqllogictest files). Where the source says what
its engine answered, SQLite's answer is compared with it: the same answer means the statement's
meaning carried across; another is kept for triage.

Sources:
  duckdb      DuckDB's sqllogictest files (test/sql): rows of tab-separated values; a file needing an
              extension, another database or a restart, or looping over templates, is left out
  cockroach   CockroachDB's logic tests (pkg/sql/logictest/testdata/logic_test): PostgreSQL's dialect
"""
import logging
import json
import math
import os
import re
import sqlite3
import sys
import tempfile

import slt

SKIP_FILE = re.compile(r"^(require|load|restart|reconnect|loop|foreach|concurrentloop|endloop|mode |attach|unzip|test-env|tags )", re.M)

logging.getLogger("sqlglot").setLevel(logging.ERROR)  # its "falling back to a Command" notes, one a statement


def parse_duckdb(path):
    """The file's records, or None when it needs what SQLite can't give (an extension, a restart)."""
    text = open(path, encoding="utf-8", errors="replace").read()
    if SKIP_FILE.search(text):
        return None
    lines = text.split("\n")
    out, i, n, cond = [], 0, len(lines), []
    while i < n:
        line = lines[i]
        if not line.strip() or line.startswith("#"):
            i += 1
            continue
        w = line.split()
        if w[0] in ("skipif", "onlyif"):
            cond.append((w[0], w[1] if len(w) > 1 else ""))
            i += 1
            continue
        if w[0] in ("statement", "query"):
            rec = {"kind": w[0], "line": i + 1, "cond": cond}
            cond = []
            if w[0] == "statement":
                rec["expect"] = w[1] if len(w) > 1 else "ok"
            else:
                rec["types"] = w[1] if len(w) > 1 else "T"
                rec["sort"] = w[2] if len(w) > 2 and w[2] in ("rowsort", "valuesort", "nosort", "sort") else "nosort"
            i += 1
            sql = []
            while i < n and lines[i].strip() and lines[i] != "----":
                sql.append(lines[i])
                i += 1
            rec["sql"] = "\n".join(sql)
            if i < n and lines[i] == "----":
                i += 1
                res = []
                while i < n and lines[i].strip():
                    res.append(lines[i])
                    i += 1
                if rec["kind"] == "query":
                    m = re.fullmatch(r"(\d+) values hashing to ([0-9a-f]{32})", res[0]) if len(res) == 1 else None
                    rec["expected"] = ("hash", int(m.group(1)), m.group(2)) if m else [r.split("\t") for r in res]
            out.append(rec)
            continue
        i += 1
    return out


def parse_cockroach(path):
    """CockroachDB's logic test records: statements, queries with space-separated row values."""
    text = open(path, encoding="utf-8", errors="replace").read()
    lines = text.split("\n")
    out, i, n = [], 0, len(lines)
    while i < n:
        line = lines[i]
        if not line.strip() or line.startswith("#"):
            i += 1
            continue
        w = line.split()
        if w[0] in ("statement", "query"):
            rec = {"kind": w[0], "line": i + 1, "cond": []}
            if w[0] == "statement":
                rec["expect"] = "error" if len(w) > 1 and w[1] in ("error", "count") and w[1] == "error" else "ok"
            else:
                rec["types"] = w[1] if len(w) > 1 else "T"
                rec["sort"] = next((x for x in w[2:] if x in ("rowsort", "valuesort", "nosort")), "nosort")
            i += 1
            sql = []
            while i < n and lines[i].strip() and lines[i] != "----":
                sql.append(lines[i])
                i += 1
            rec["sql"] = "\n".join(sql)
            if i < n and lines[i] == "----":
                i += 1
                res = []
                while i < n and lines[i].strip():
                    res.append(lines[i])
                    i += 1
                if rec["kind"] == "query":
                    rec["expected_lines"] = res
            out.append(rec)
            continue
        i += 1
    return out


def split_pg(text):
    """A psql script's statements: semicolons outside quotes ('', E'', $tag$...$tag$), comments
    (--, nested /* */) and psql's backslash commands; COPY ... FROM stdin's data is skipped."""
    out, cur, i, n = [], [], 0, len(text)
    while i < n:
        c = text[i]
        if c == "-" and text.startswith("--", i):
            j = text.find("\n", i)
            i = n if j < 0 else j
            continue
        if c == "/" and text.startswith("/*", i):
            depth, i = 1, i + 2
            while i < n and depth:
                if text.startswith("/*", i):
                    depth, i = depth + 1, i + 2
                elif text.startswith("*/", i):
                    depth, i = depth - 1, i + 2
                else:
                    i += 1
            continue
        if c == "\\" and (i == 0 or text[i - 1] == "\n"):
            j = text.find("\n", i)
            i = n if j < 0 else j
            if "".join(cur).strip():
                out.append("".join(cur).strip())
            cur = []
            continue
        if c == "'":
            escaped = i > 0 and text[i - 1] in "eE" and (i < 2 or not text[i - 2].isalnum())
            j = i + 1
            while j < n:
                if escaped and text[j] == "\\":
                    j += 2
                    continue
                if text[j] == "'":
                    if j + 1 < n and text[j + 1] == "'":
                        j += 2
                        continue
                    break
                j += 1
            cur.append(text[i:j + 1])
            i = j + 1
            continue
        if c == "$":
            m = re.match(r"\$([A-Za-z_][A-Za-z_0-9]*)?\$", text[i:])
            if m:
                tag = m.group(0)
                j = text.find(tag, i + len(tag))
                j = n if j < 0 else j + len(tag)
                cur.append(text[i:j])
                i = j
                continue
        if c == ";":
            stmt = "".join(cur).strip()
            if stmt:
                out.append(stmt)
            cur = []
            i += 1
            if re.match(r"(?is)\s*COPY\b.*\bFROM\s+stdin", stmt):
                j = text.find("\n\\.", i)
                i = n if j < 0 else j + 3
            continue
        cur.append(c)
        i += 1
    if "".join(cur).strip():
        out.append("".join(cur).strip())
    return out


def parse_postgres(path):
    """A regression script's statements, in order, each a statement or a query by what SQLite says."""
    text = open(path, encoding="utf-8", errors="replace").read()
    return [{"kind": "auto", "sql": s, "line": text[:text.find(s[:40])].count("\n") + 1 if s[:40] in text else 0, "cond": [], "sort": "rowsort"} for s in split_pg(text)]


def normal(v):
    """A value as the sources print theirs, for comparing: numbers as numbers, booleans as 0/1."""
    if v is None:
        return None
    if isinstance(v, (bytes, bytearray)):
        return bytes(v).hex()
    if isinstance(v, bool):
        return float(v)
    if isinstance(v, (int, float)):
        return float(v)
    s = str(v)
    if s in ("NULL",):
        return None
    if s == "(empty)":
        return ""
    if s.lower() in ("true", "false"):
        return 1.0 if s.lower() == "true" else 0.0
    try:
        return float(s)
    except ValueError:
        return s


def same(a, b):
    if isinstance(a, float) and isinstance(b, float):
        return a == b or (math.isfinite(a) and math.isfinite(b) and abs(a - b) <= 1e-6 * max(1.0, abs(a), abs(b)))
    return a == b


def compare(rec, rows):
    """SQLite's rows against what the source's engine answered: "agree", "disagree", or None when
    the source gave no comparable answer."""
    if "expected" in rec:
        exp = rec["expected"]
        if isinstance(exp, tuple):
            return None
        ncols = len(rec.get("types", "T"))
        # DuckDB's runner takes a row a line, or (as SQLite's does) a value a line.
        if ncols > 1 and exp and all(len(r) == 1 for r in exp) and len(exp) % ncols == 0:
            flat = [r[0] for r in exp]
            exp = [flat[i:i + ncols] for i in range(0, len(flat), ncols)]
        got = [[normal(v) for v in r] for r in rows]
        want = [[normal(v) for v in r] for r in exp]
        if rec["sort"] in ("rowsort", "sort"):
            key = lambda r: json.dumps([("" if v is None else str(v)) for v in r])
            got, want = sorted(got, key=key), sorted(want, key=key)
        elif rec["sort"] == "valuesort":
            got = [sorted((v for r in got for v in r), key=lambda v: str(v))]
            want = [sorted((v for r in want for v in r), key=lambda v: str(v))]
        if len(got) != len(want) or any(len(x) != len(y) or not all(same(p, q) for p, q in zip(x, y)) for x, y in zip(got, want)):
            return "disagree"
        return "agree"
    if "expected_lines" in rec:
        want = rec["expected_lines"]
        got = [" ".join("NULL" if v is None else str(v) for v in r) for r in rows]
        if rec["sort"] == "rowsort":
            got, want = sorted(got), sorted(want)
        flat_got = " ".join(got).split()
        flat_want = " ".join(want).split()
        if len(flat_got) != len(flat_want):
            return "disagree"
        return "agree" if all(same(normal(p), normal(q)) for p, q in zip(flat_got, flat_want)) else "disagree"
    return None


WHY = {  # each cause a disagreement can have, as the triage names it
    "order": "the same rows in another order (no ORDER BY, or ties under it)",
    "plan": "a plan (EXPLAIN): each engine's own",
    "catalog": "the engine's own catalog (PRAGMAs, information_schema, pg_catalog, duckdb_*, crdb_internal, SHOW)",
    "rows": "a row count from data the translation couldn't carry: rows a COPY from a file, a statement SQLite refused, or another session made",
    "type": "a type SQLite doesn't have (wide and unsigned integers, DECIMAL, intervals, time zones, UUID, BIT, ENUM, nested types): SQLite reads the value by affinity",
    "datetime": "dates and times: SQLite keeps them as text or numbers, with functions of its own",
    "division": "division: the source divides integers into a fraction, SQLite into an integer",
    "source refused": "the source engine refuses what SQLite accepts (its type checks, its errors for overflow, division by zero, bad input)",
    "aborted transaction": "PostgreSQL refused it only because an earlier error in the script aborted its transaction",
    "semantics": "the same SQL means something else on SQLite (affinity, NULL and type rules, a function's own semantics)",
}
WHY_PLAN = re.compile(r"^\s*EXPLAIN\b", re.I)
WHY_CATALOG = re.compile(r"\bPRAGMA\b|\bduckdb_\w+|information_schema|pg_catalog|\bpg_[a-z_]+\s*\(|crdb_internal|^\s*SHOW\b|sqlite_(schema|master)", re.I)
WHY_TYPE = re.compile(r"\b(HUGEINT|UHUGEINT|U?INT(8|16|32|64|128)|UBIGINT|USMALLINT|UTINYINT|UINTEGER|BIGNUM|VARINT|BIT|BITSTRING|ENUM|UUID|INTERVAL|TIMETZ|TIMESTAMPTZ"
                      r"|STRUCT|MAP|UNION|LIST|DECIMAL|NUMERIC|GEOMETRY|JSONB?|INET|BYTEA|MONEY|OID|REGCLASS|BOX|POINT|FLOAT4|REAL|DOUBLE|FLOAT)\b|WITH\s+TIME\s+ZONE", re.I)
WHY_DATETIME = re.compile(r"\b(DATE|TIME|TIMESTAMP|STRFTIME|STRPTIME|DATE_PART|DATEPART|DATE_TRUNC|DATE_DIFF|DATEDIFF|EXTRACT|AGE|EPOCH|NOW|MAKE_DATE|MAKE_TIMESTAMP"
                          r"|TO_TIMESTAMP|TO_CHAR|TO_DATE|DAYNAME|MONTHNAME|YEAR|MONTH|DAY|HOUR|MINUTE|SECOND)\b", re.I)


def classify(rec, rows):
    """Why SQLite's answer and the source engine's differ, read from the record and both answers: the
    first rule that fits (WHY names each)."""
    src = rec.get("sql", "")
    got, want = [normal(v) for r in rows for v in r], None
    if "expected" in rec and not isinstance(rec["expected"], tuple):
        want = [normal(v) for r in rec["expected"] for v in r]
    elif "expected_lines" in rec:
        want = [normal(v) for v in " ".join(rec["expected_lines"]).split()]
        got = [normal(v) for v in " ".join(" ".join("NULL" if v is None else str(v) for v in r) for r in rows).split()]
    key = lambda v: (str(type(v)), str(v))
    if want is not None and len(got) == len(want) and all(same(p, q) for p, q in zip(sorted(got, key=key), sorted(want, key=key))):
        return "order"
    if WHY_PLAN.search(src):
        return "plan"
    if WHY_CATALOG.search(src):
        return "catalog"
    if want is not None and len(got) != len(want) and (not got or not want):
        return "rows"
    if WHY_TYPE.search(src):
        return "type"
    if WHY_DATETIME.search(src):
        return "datetime"
    if want is not None and "/" in src and any(isinstance(p, float) and p.is_integer() and isinstance(q, float) and not q.is_integer() for p, q in zip(got, want)):
        return "division"
    return "semantics"


def run(path, records, read, keep):
    """Runs a parsed file on SQLite: returns counts and the sqllogictest file it becomes."""
    import sqlglot
    fd, db = tempfile.mkstemp(prefix="corpus-", suffix=".db", dir=os.path.expanduser("~/.cache"))
    os.close(fd)
    con = slt.connect(db, text=str)
    counts = {"records": 0, "untranslatable": 0, "sqlite_refused": 0, "kept": 0, "agree": 0, "disagree": 0, "not_compared": 0}
    disagreements = []
    lines = ["hash-threshold 8", ""]
    try:
        for rec in records:
            counts["records"] += 1
            if any(c == "onlyif" and e not in ("", "sqlite") for c, e in rec["cond"]):
                continue
            try:
                with slt.budget():
                    parts = sqlglot.transpile(rec["sql"], read=read, write="sqlite", unsupported_level=sqlglot.ErrorLevel.RAISE)
            except Exception:
                key = "over_budget" if slt.spent() else "untranslatable"
                counts[key] = counts.get(key, 0) + 1
                continue
            parts = [slt.record_sql(p_) for p_ in parts]
            if not parts:
                continue
            if None in parts:  # a blank line inside a literal: no record carries it
                counts["unrepresentable"] = counts.get("unrepresentable", 0) + 1
                continue
            sql = ";\n".join(parts)
            done = 0
            try:
                with slt.budget():
                    rows = []
                    for part in parts:
                        cur = con.execute(part)
                        rows = slt.fetch(cur) if cur.description else []
                        done += 1
                ok = True
            except (sqlite3.Error, slt.OverBudget):
                ok = False
            if slt.spent():
                # cut at its budget: the part SQLite was stopped in rolled back, so the file keeps
                # only the parts before it (the database is as this run left it)
                counts["over_budget"] = counts.get("over_budget", 0) + 1
                lines += sum((["statement ok", p_, ""] for p_ in parts[:done]), [])
                continue
            if not ok and done:
                # the parts before the one SQLite refused ran: kept, so the file leaves the database
                # as this run did
                counts["sqlite_refused"] += 1
                counts["kept_partly"] = counts.get("kept_partly", 0) + 1
                lines += sum((["statement ok", p_, ""] for p_ in parts[:done]), []) + ["statement error", parts[done], ""]
                keep(rec, ";\n".join(parts[:done + 1]))
                continue
            if rec["kind"] == "statement":
                # A statement the source expects to fail stays in when SQLite refuses it too.
                if not ok and rec.get("expect") != "error":
                    counts["sqlite_refused"] += 1
                    continue
                counts["kept"] += 1
                lines += [f"statement {'ok' if ok else 'error'}", sql, ""]
                keep(rec, sql)
                continue
            if not ok:
                counts["sqlite_refused"] += 1
                continue
            counts["kept"] += 1
            if cur.description is None or not slt.stable(sql):
                # the translation answers nothing, or answers differently each run (random(), the
                # clock): a statement on SQLite, its answer not compared
                key = "not_compared" if cur.description is None else "unstable"
                counts[key] = counts.get(key, 0) + 1
                lines += ["statement ok", sql, ""]
                keep(rec, sql)
                continue
            verdict = compare(rec, rows)
            counts[{"agree": "agree", "disagree": "disagree", None: "not_compared"}[verdict]] += 1
            if verdict == "disagree":
                why = classify(rec, rows)
                counts[f"why: {why}"] = counts.get(f"why: {why}", 0) + 1
                if len(disagreements) < 50:
                    expected = rec.get("expected") if "expected" in rec else rec.get("expected_lines")
                    disagreements.append([rec["line"], why, sql[:300], json.dumps(rows[:4], default=str)[:200], json.dumps(expected[:4] if isinstance(expected, list) else expected, default=str)[:200]])
            # the source's type letters, as sqllogictest's (I, R, T); SQLite's own when the
            # translation changed the column count
            types = "".join(t if t in "IRT" else "T" for t in rec.get("types", "T"))
            if len(types) != len(cur.description):
                types = slt.column_types(rows, len(cur.description))
            sort = rec["sort"] if rec["sort"] in ("rowsort", "valuesort") else "nosort"
            vals = slt.answer(rows, types, sort)
            body = [f"{len(vals)} values hashing to {slt.md5_of(vals)}"] if len(vals) > 8 or any(not v.strip() for v in vals) else vals
            lines += sum((["statement ok", p_, ""] for p_ in parts[:-1]), []) + [f"query {types} {sort}", parts[-1], "----"] + body + [""]
            keep(rec, sql)
    finally:
        con.close()
        os.remove(db)
    return counts, disagreements, "\n".join(lines)


def copy_files(setup_text):
    """test_setup.sql's COPY statements and the data file each reads (from its \\set filename)."""
    out, current = {}, None
    for line in setup_text.split("\n"):
        m = re.match(r"\\set filename :abs_srcdir '/data/([\w.]+)'", line.strip())
        if m:
            current = m.group(1)
        m = re.match(r"COPY (\w+) FROM :'filename'", line.strip(), re.I)
        if m and current:
            out[m.group(1).lower()] = current
    return out


def data_rows(path):
    """A COPY data file's rows: tab-separated values, \\N for NULL, backslash escapes."""
    rows = []
    for line in open(path, encoding="utf-8", errors="replace"):
        line = line.rstrip("\n")
        if not line or line == "\\.":
            continue
        rows.append([None if v == "\\N" else v.replace("\\t", "\t").replace("\\\\", "\\") for v in line.split("\t")])
    return rows


def sql_literal(v):
    return "NULL" if v is None else "'" + v.replace("'", "''") + "'"


def pg_base(regress, sqlite_db):
    """The regression suite's base tables made in a SQLite database (test_setup.sql translated, its
    data files read in as INSERTs); returns the setup's statements and COPY sources, for
    PostgreSQL's turn, and the fixture: the statements that made the database, as a sqllogictest
    file each of the suite's files runs first."""
    import sqlglot
    text = open(os.path.join(regress, "sql", "test_setup.sql"), encoding="utf-8").read()
    copies = copy_files(text)
    con = slt.connect(sqlite_db, text=str)
    fixture = ["hash-threshold 8", "", "# PostgreSQL's regression base tables (test_setup.sql and its data files) as SQLite makes them: run first", ""]
    statements = split_pg(text)
    for st in statements:
        m = re.match(r"COPY (\w+) FROM :'filename'", st, re.I)
        if m:
            table = m.group(1).lower()
            rows = data_rows(os.path.join(regress, "data", copies.get(table, ""))) if copies.get(table) else []
            for k in range(0, len(rows), 200):
                sql = f"INSERT INTO {table} VALUES " + ", ".join("(" + ", ".join(sql_literal(v) for v in r) + ")" for r in rows[k:k + 200])
                try:
                    con.execute(sql)
                    fixture += ["statement ok", sql, ""]
                except sqlite3.Error:
                    pass
            continue
        try:
            parts = sqlglot.transpile(st, read="postgres", write="sqlite", unsupported_level=sqlglot.ErrorLevel.RAISE)
        except Exception:
            continue
        for part in parts:
            part = slt.record_sql(part)
            if part is None:
                continue
            try:
                con.execute(part).fetchall()
                fixture += ["statement ok", part, ""]
            except sqlite3.Error:
                pass
    con.close()
    return statements, copies, "\n".join(fixture)


PG_TEMPLATE = "corpus_base"
PG_SKIP = re.compile(r"(?i)\s*(CREATE\s+TABLESPACE|CREATE\s+(OR\s+REPLACE\s+)?FUNCTION\s+\w+.*:'regresslib')")


def pg_template(regress, pg_dsn, setup, copies):
    """The suite's base tables (test_setup.sql and its data files) in a PostgreSQL database each
    script's own database is cloned from. Returns how many of the setup's statements ran."""
    import psycopg
    from psycopg.conninfo import make_conninfo
    with psycopg.connect(pg_dsn, autocommit=True) as admin:
        admin.execute(f"DROP DATABASE IF EXISTS {PG_TEMPLATE} WITH (FORCE)")
        admin.execute(f"CREATE DATABASE {PG_TEMPLATE}")
    made = 0
    with psycopg.connect(make_conninfo(pg_dsn, dbname=PG_TEMPLATE), autocommit=True) as pg:
        for st in setup:
            m = re.match(r"COPY (\w+) FROM :'filename'", st, re.I)
            try:
                if m:
                    data = open(os.path.join(regress, "data", copies[m.group(1).lower()]), "rb").read()
                    with pg.cursor().copy(f"COPY {m.group(1)} FROM STDIN") as cp:
                        cp.write(data)
                elif not PG_SKIP.match(st):
                    pg.execute(st)
                made += 1
            except Exception:
                pass  # what PostgreSQL refuses of the setup, its scripts meet as they do on SQLite
    return made


def run_postgres(path, regress, base_db, setup, copies, pg_dsn, keep):
    """One regression script: each statement translated and run on a copy of the base database;
    each SQLite runs, run as written on PostgreSQL too, in a database of the script's own cloned from
    the base tables' (pg_template), and the answers compared."""
    import shutil
    import sqlglot
    fd, db = tempfile.mkstemp(prefix="corpus-pg-", suffix=".db", dir=os.path.expanduser("~/.cache"))
    os.close(fd)
    shutil.copyfile(base_db, db)
    con = slt.connect(db, text=str)
    pg = admin = None
    if pg_dsn:
        import psycopg
        from psycopg.conninfo import make_conninfo
        name = f"corpus_{os.getpid()}_{abs(hash(path)) % 10**8}"
        admin = psycopg.connect(pg_dsn, autocommit=True)
        admin.execute(f'DROP DATABASE IF EXISTS "{name}" WITH (FORCE)')
        admin.execute(f'CREATE DATABASE "{name}" TEMPLATE {PG_TEMPLATE}')
        # the database's own defaults, so a script's RESET ALL keeps them: one waiting on a lock
        # doesn't hold the run
        admin.execute(f"ALTER DATABASE \"{name}\" SET statement_timeout = '30s'")
        admin.execute(f"ALTER DATABASE \"{name}\" SET lock_timeout = '10s'")
        pg = psycopg.connect(make_conninfo(pg_dsn, dbname=name), autocommit=True)
    counts = {"records": 0, "untranslatable": 0, "sqlite_refused": 0, "kept": 0, "agree": 0, "disagree": 0, "not_compared": 0}
    disagreements = []
    lines = ["hash-threshold 8", "", "# fixture: base.test (run first, on the same database)", ""]
    try:
        for rec in parse_postgres(path):
            counts["records"] += 1
            try:
                with slt.budget():
                    parts = sqlglot.transpile(rec["sql"], read="postgres", write="sqlite", unsupported_level=sqlglot.ErrorLevel.RAISE)
            except Exception:
                key = "over_budget" if slt.spent() else "untranslatable"
                counts[key] = counts.get(key, 0) + 1
                continue
            parts = [slt.record_sql(p_) for p_ in parts]
            if not parts:
                continue
            if None in parts:
                counts["unrepresentable"] = counts.get("unrepresentable", 0) + 1
                continue
            sql = ";\n".join(parts)
            done = 0
            try:
                with slt.budget():
                    rows, desc = [], None
                    for part in parts:
                        cur = con.execute(part)
                        desc = cur.description
                        rows = slt.fetch(cur) if desc else []
                        done += 1
            except (sqlite3.Error, slt.OverBudget):
                if slt.spent():  # cut at its budget: the parts before the one stopped stay
                    counts["over_budget"] = counts.get("over_budget", 0) + 1
                    lines += sum((["statement ok", p_, ""] for p_ in parts[:done]), [])
                    continue
                counts["sqlite_refused"] += 1
                if done:  # the parts that ran stay in the file, so it leaves the database as this run did
                    counts["kept_partly"] = counts.get("kept_partly", 0) + 1
                    lines += sum((["statement ok", p_, ""] for p_ in parts[:done]), []) + ["statement error", parts[done], ""]
                    keep(rec, ";\n".join(parts[:done + 1]))
                continue
            counts["kept"] += 1
            if desc is not None and not slt.stable(sql):  # random() or the clock: run, not compared
                counts["unstable"] = counts.get("unstable", 0) + 1
                lines += ["statement ok", sql, ""]
                keep(rec, sql)
                continue
            verdict = None
            why = None
            if pg is not None:
                pg_error = None
                try:
                    cur = pg.execute(rec["sql"])
                    pg_rows = slt.fetch(cur) if cur.description else None
                except Exception as e:
                    pg_rows, pg_error = "error", type(e).__name__
                pg_rec = {"sql": rec["sql"], "sort": "rowsort", "types": "T" * len(desc or ())}
                if isinstance(pg_rows, list):
                    pg_rec["expected"] = [[("NULL" if v is None else v) for v in r] for r in pg_rows]
                if pg_rows == "error":
                    verdict = "disagree"
                    why = "aborted transaction" if pg_error == "InFailedSqlTransaction" else "source refused"
                elif desc is None:
                    verdict = "agree"
                else:
                    verdict = compare(pg_rec, rows) if isinstance(pg_rows, list) else "disagree"
                    if verdict == "disagree":
                        why = classify(pg_rec, rows) if isinstance(pg_rows, list) else "semantics"
            counts[{"agree": "agree", "disagree": "disagree", None: "not_compared"}[verdict]] += 1
            if verdict == "disagree":
                counts[f"why: {why}"] = counts.get(f"why: {why}", 0) + 1
                if len(disagreements) < 50:
                    disagreements.append([rec["line"], why, rec["sql"][:200], sql[:200], json.dumps(rows[:3], default=str)[:160],
                                          json.dumps(pg_rec.get("expected", pg_rows)[:3] if isinstance(pg_rows, list) else pg_error, default=str)[:160]])
            if desc is None:
                lines += ["statement ok", sql, ""]
            else:
                types = slt.column_types(rows, len(desc))
                vals = slt.answer(rows, types, "rowsort")
                body = [f"{len(vals)} values hashing to {slt.md5_of(vals)}"] if len(vals) > 8 or any(not v.strip() for v in vals) else vals
                lines += sum((["statement ok", p_, ""] for p_ in parts[:-1]), []) + [f"query {types} rowsort", parts[-1], "----"] + body + [""]
            keep(rec, sql)
    finally:
        con.close()
        os.remove(db)
        if pg is not None:
            pg.close()
        if admin is not None:
            try:  # whatever state the script left its database in
                admin.execute(f'DROP DATABASE IF EXISTS "{name}" WITH (FORCE)')
            except Exception as e:
                print(f"{path}: its database {name} is left: {e}", file=sys.stderr)
            admin.close()
    return counts, disagreements, "\n".join(lines)
