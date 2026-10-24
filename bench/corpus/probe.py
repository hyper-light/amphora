#!/usr/bin/env python3
"""Runs the probes (probes.py) on one engine and writes each probe's answer or error, so the
support matrix (matrix.py) compares every engine with SQLite, entry by entry.

Engines: sqlite (Python's sqlite3, the reference), duckdb (its Python module); postgres, orioledb
and cedardb (psycopg), mysql (pymysql), clickhouse (its HTTP interface). Every engine but SQLite
takes each probe in its own dialect: the hand-written spelling for it in spellings.json when there
is one, else sqlglot's translation from SQLite's dialect (each result says which, under
`spelling`). Amphora and Turso run the probes through the bench (`amphora-bench --probe`).

A server that dies under a probe is restarted (--restart, the bench's servers/<engine>.sh start)
and the run goes on: that probe is recorded as `crash`, and a probe that finds the server already
gone is tried again once it's back.

usage: probe.py <engine> <probes.json> <out.json> [--dsn ...] [--restart '<command>'] [--spellings spellings.json]
"""
import json
import math
import os
import re
import shlex
import sqlite3
import subprocess
import sys
import tempfile
import time

ENGINE, PROBES, OUT = sys.argv[1], sys.argv[2], sys.argv[3]


def arg(name):
    return sys.argv[sys.argv.index(name) + 1] if name in sys.argv else None


DSN = arg("--dsn")
RESTART = arg("--restart")
SPELLINGS = json.load(open(arg("--spellings"))) if arg("--spellings") else {}
DIALECT = {"sqlite": "sqlite", "duckdb": "duckdb", "postgres": "postgres", "orioledb": "postgres", "cedardb": "postgres", "mysql": "mysql", "clickhouse": "clickhouse"}


class ServerGone(Exception):
    """The server closed the connection or refused one: it died or isn't up."""


def norm(v):
    """One spelling per value across engines: integers as integers, reals to 9 significant digits,
    booleans as 0/1, bytes as hex, everything else as text."""
    if v is None:
        return None
    if isinstance(v, bool):
        return int(v)
    if isinstance(v, int):
        return v
    if isinstance(v, float):
        if math.isnan(v):
            return "NaN"
        if math.isinf(v):
            return "Inf" if v > 0 else "-Inf"
        if v == int(v) and abs(v) < 2**53:
            return int(v)
        return float(f"{v:.9g}")
    if isinstance(v, (bytes, bytearray, memoryview)):
        return "x'" + bytes(v).hex() + "'"
    try:
        import decimal
        if isinstance(v, decimal.Decimal):
            return norm(float(v))
    except ImportError:
        pass
    return str(v)


def translate(sql):
    if DIALECT[ENGINE] == "sqlite":
        return sql, None
    import sqlglot
    try:
        out = sqlglot.transpile(sql, read="sqlite", write=DIALECT[ENGINE], unsupported_level=sqlglot.ErrorLevel.RAISE)
    except Exception as e:  # sqlglot can't say it in this dialect
        return None, f"untranslatable: {type(e).__name__}: {str(e)[:200]}"
    if DIALECT[ENGINE] == "clickhouse":
        # A primary key orders a MergeTree table, and its columns can't be Nullable there.
        out = [re.sub(r"Nullable\(([A-Za-z0-9]+(?:\([0-9, ]+\))?)\)(\s+PRIMARY KEY)", r"\1\2", o) for o in out]
    if DIALECT[ENGINE] == "mysql":
        # SQLite's TEXT column is MySQL's VARCHAR: MySQL's TEXT takes no DEFAULT and no index
        # without a prefix length, which SQLite's text has no need of.
        out = [re.sub(r"\bTEXT\b", "VARCHAR(255)", o) if re.match(r"\s*(CREATE\s+(TEMPORARY\s+)?TABLE|ALTER\s+TABLE)", o, re.I) else o for o in out]
    return ";\n".join(out), None


class Sqlite:
    def __init__(self):
        self.dir = tempfile.mkdtemp(prefix="probe-", dir=os.path.expanduser("~/.cache"))
        self.n = 0

    def version(self):
        return f"SQLite {sqlite3.sqlite_version} (Python's sqlite3)"

    def fresh(self):
        self.n += 1
        # timeout=0: SQLite's own default busy timeout (the module's default is five seconds).
        c = sqlite3.connect(os.path.join(self.dir, f"{self.n}.db"), isolation_level=None, timeout=0)
        # X REGEXP Y calls regexp(Y, X), which SQLite leaves to the application; this is the one the
        # sqlite3 shell registers in effect (its regexp extension), for patterns all flavors share.
        c.create_function("regexp", 2, lambda pat, val: None if pat is None or val is None else int(re.search(pat, str(val)) is not None), deterministic=True)
        return c

    def run(self, c, sql, params=None):
        cur = c.cursor()
        rows = []
        # A probe may hold several statements; the answer is the last one's.
        for part in _split(sql):
            cur.execute(part, params if params is not None else ())
            rows = cur.fetchall() if cur.description else []
        return rows

    def close(self, c):
        c.close()

    def cleanup(self):
        for f in os.listdir(self.dir):
            os.remove(os.path.join(self.dir, f))
        os.rmdir(self.dir)


class Duck(Sqlite):
    def version(self):
        import duckdb
        return f"DuckDB {duckdb.__version__} (Python module)"

    def fresh(self):
        import duckdb
        self.n += 1
        return duckdb.connect(os.path.join(self.dir, f"{self.n}.duckdb"))

    def run(self, c, sql, params=None):
        rows = []
        for part in _split(sql):
            r = c.execute(part, params) if params is not None else c.execute(part)
            try:
                rows = r.fetchall()
            except Exception:
                rows = []
        return rows


class Pg:
    def __init__(self):
        import psycopg
        self.psycopg = psycopg
        self.n = 0

    def connect(self):
        try:
            return self.psycopg.connect(DSN, autocommit=True, connect_timeout=10)
        except self.psycopg.OperationalError as e:
            raise ServerGone(str(e)) from e

    def version(self):
        c = self.connect()
        try:
            return c.execute("SELECT version()").fetchone()[0]
        finally:
            c.close()

    def fresh(self):
        # Each probe in a schema of its own, dropped after.
        self.n += 1
        c = self.connect()
        s = f"probe_{os.getpid()}_{self.n}"
        c.execute(f"DROP SCHEMA IF EXISTS {s} CASCADE")
        c.execute(f"CREATE SCHEMA {s}")
        c.execute(f"SET search_path TO {s}")
        c.schema = s
        return c

    def run(self, c, sql):
        rows = []
        for part in _split(sql):
            try:
                cur = c.execute(part)
            except self.psycopg.OperationalError as e:
                if c.closed or c.broken:
                    raise ServerGone(str(e)) from e
                raise
            rows = cur.fetchall() if cur.description else []
        return rows

    def close(self, c):
        try:
            if not (c.closed or c.broken):
                c.execute(f"DROP SCHEMA IF EXISTS {c.schema} CASCADE")
        finally:
            c.close()


class My:
    # The client's codes for a connection that broke or never opened.
    GONE = {2002, 2003, 2006, 2013, 2055}

    def __init__(self):
        import pymysql
        self.pymysql = pymysql
        from urllib.parse import urlparse
        u = urlparse(DSN)
        self.kw = dict(host=u.hostname, port=u.port or 3306, user=u.username or "root", password=u.password or "", autocommit=True, connect_timeout=10)
        self.n = 0

    def connect(self):
        try:
            return self.pymysql.connect(**self.kw)
        except self.pymysql.err.OperationalError as e:
            raise ServerGone(str(e)) from e

    def version(self):
        c = self.connect()
        try:
            with c.cursor() as cur:
                cur.execute("SELECT version()")
                return "MySQL " + cur.fetchone()[0]
        finally:
            c.close()

    def fresh(self):
        self.n += 1
        c = self.connect()
        d = f"probe_{os.getpid()}_{self.n}"
        with c.cursor() as cur:
            cur.execute(f"DROP DATABASE IF EXISTS {d}")
            cur.execute(f"CREATE DATABASE {d}")
            cur.execute(f"USE {d}")
        c.db_name = d
        return c

    def run(self, c, sql):
        rows = []
        with c.cursor() as cur:
            for part in _split(sql):
                try:
                    cur.execute(part)
                except self.pymysql.err.OperationalError as e:
                    if e.args and e.args[0] in self.GONE:
                        raise ServerGone(str(e)) from e
                    raise
                rows = list(cur.fetchall()) if cur.description else []
        return rows

    def close(self, c):
        try:
            with c.cursor() as cur:
                cur.execute(f"DROP DATABASE IF EXISTS {c.db_name}")
        finally:
            c.close()


class Ch:
    def __init__(self):
        import requests
        self.requests = requests
        self.url = DSN.rstrip("/")
        self.n = 0

    def version(self):
        return "ClickHouse " + self.q("SELECT version()").strip().splitlines()[-1]

    def fresh(self):
        self.n += 1
        d = f"probe_{os.getpid()}_{self.n}"
        self.q(f"DROP DATABASE IF EXISTS {d}")
        self.q(f"CREATE DATABASE {d}")
        return d

    def q(self, sql, db=None, fmt="TSV"):
        # 64-bit integers as JSON numbers, not quoted strings (ClickHouse's default quotes them).
        params = {"default_format": fmt, "output_format_json_quote_64bit_integers": 0}
        if db:
            params["database"] = db
        try:
            r = self.requests.post(self.url, params=params, data=sql.encode(), timeout=60)
        except self.requests.ConnectionError as e:
            raise ServerGone(str(e)) from e
        if r.status_code != 200:
            raise RuntimeError(r.text[:300])
        return r.text

    def run(self, db, sql):
        rows = []
        for part in _split(sql):
            text = self.q(part, db, fmt="JSONCompact")
            rows = json.loads(text)["data"] if text.strip().startswith("{") else []
        return rows

    def close(self, db):
        self.q(f"DROP DATABASE IF EXISTS {db}")


def _split(sql):
    """Statements of a probe; a trigger's body keeps its semicolons (BEGIN ... END), as does a
    dollar-quoted function body (a hand-written PostgreSQL spelling)."""
    if "$$" in sql:
        return [sql]
    if sqlite3.complete_statement(sql if sql.rstrip().endswith(";") else sql + ";") and "BEGIN" in sql.upper() and "END" in sql.upper() and "TRIGGER" in sql.upper():
        return [sql]
    out, cur = [], ""
    for piece in sql.split(";"):
        cur = cur + piece if not cur else cur + ";" + piece
        if sqlite3.complete_statement(cur + ";"):
            if cur.strip():
                out.append(cur.strip())
            cur = ""
    if cur.strip():
        out.append(cur.strip())
    return out


def engine():
    return {"sqlite": Sqlite, "duckdb": Duck, "postgres": Pg, "orioledb": Pg, "cedardb": Pg, "mysql": My, "clickhouse": Ch}[ENGINE]()


def restart(e):
    """Brings a dead server back with the bench's own script, then waits until it takes a connection
    (its script's readiness check runs from another container), five minutes at most: a server
    recovering a large database after a crash takes minutes; false when it doesn't come back."""
    if not RESTART:
        return False
    # A server's container can still count as running for seconds after its server died, and the
    # script leaves a running container be: start it again until it takes a connection.
    deadline = time.time() + 300
    while time.time() < deadline:
        subprocess.run(shlex.split(RESTART), capture_output=True, text=True, timeout=600)
        for _ in range(40):
            try:
                e.version()
                return True
            except Exception:
                time.sleep(0.5)
    return False


def spelled(p):
    """The probe in this engine's dialect: (setup, query, how) or an error (status, message)."""
    if DIALECT[ENGINE] == "sqlite":
        return (p.get("sqlite_setup", []) + p["setup"], p["query"], "sqlite"), None
    rule = SPELLINGS.get(p["id"], {}).get(DIALECT[ENGINE])
    if isinstance(rule, str):
        # A rule that says there's no way to say it in this dialect, and why.
        return None, ("no equivalent", rule)
    setup, query, how = None, None, "sqlglot"
    if rule:
        # A rule may give the whole setup, or only the probe's own statements after the shared
        # base tables (`setup_tail`), and the query.
        setup, query, how = rule.get("setup"), rule.get("query"), "rule"
    if setup is None:
        own = p["setup"][p.get("base_len", 0):]
        base = p["setup"][:p.get("base_len", 0)]
        setup = []
        for s in base + ([] if rule and "setup_tail" in rule else own):
            t, err = translate(s)
            if err:
                return None, ("untranslatable", err + " (setup)")
            setup.append(t)
        if rule and "setup_tail" in rule:
            setup += rule["setup_tail"]
    if query is None:
        query, err = translate(p["query"])
        if err:
            return None, ("untranslatable", err)
    return (setup, query, how), None


def attempt(e, p, setup, query):
    c = None
    try:
        c = e.fresh()
        for s in setup:
            e.run(c, s)
        rows = e.run(c, query, p["params"]) if "params" in p else e.run(c, query)
        rows = [[norm(v) for v in r] for r in rows]
        if not p["ordered"]:
            rows = sorted(rows, key=lambda r: json.dumps(r, sort_keys=True))
        return {"status": "ok", "rows": rows}
    finally:
        if c is not None:
            try:
                e.close(c)
            except Exception:
                pass


def main():
    probes = json.load(open(PROBES))["probes"]
    e = engine()
    try:
        version = e.version()
    except ServerGone:
        version = e.version() if restart(e) else "unreachable"
    results, crashes = [], 0
    began = time.time()
    for p in probes:
        res = {"id": p["id"]}
        sp, err = (None, None) if "params" in p and ENGINE not in ("sqlite", "duckdb") else spelled(p)
        if "params" in p and ENGINE not in ("sqlite", "duckdb"):
            res.update(status="n/a", error="a client-side parameter syntax: each driver binds its own")
        elif err:
            res.update(status=err[0], error=err[1])
        else:
            setup, query, how = sp
            res["sql"], res["spelling"] = query, how
            if how == "rule":
                res["setup_sql"] = setup
            for tries in (1, 2):
                try:
                    res.update(attempt(e, p, setup, query))
                    break
                except ServerGone as ex:
                    # Gone under this probe, or found gone: bring it back, then try once more.
                    up = restart(e)
                    if tries == 1 and up:
                        continue
                    crashes += 1
                    why = "and didn't come back in five minutes" if tries == 1 else "again on a second try" + ("" if up else ", and didn't come back in five minutes")
                    res.update(status="crash", error=f"the server went away under this probe {why}: {str(ex)[:200]}")
                    break
                except Exception as ex:
                    res.update(status="error", error=f"{type(ex).__name__}: {str(ex)[:300]}")
                    break
        if p.get("requires"):
            res["requires"] = p["requires"]
        results.append(res)
    if hasattr(e, "cleanup"):
        e.cleanup()
    import sqlglot
    json.dump({"engine": ENGINE, "version": version, "sqlglot": sqlglot.__version__, "seconds": round(time.time() - began, 1), "crashes": crashes, "results": results}, open(OUT, "w"), indent=1)
    ok = sum(r["status"] == "ok" for r in results)
    print(f"{ENGINE}: {ok} of {len(results)} probes ran, {crashes} crashed the server ({time.time() - began:.1f} s)")


main()
