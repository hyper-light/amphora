"""dialects.py's translation (G2 step 4): what it says in each dialect, and what it gives through as
SQLite holds it. Run: python3 -m unittest bench/corpus/test_dialects.py (sqlglot as sources.json pins)."""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import dialects  # noqa: E402

ENGINES = dialects.DIALECTS


class Record:
    def __init__(self, sql, kind="statement", expect="ok"):
        self.sql, self.kind, self.expect = sql, kind, expect


class DefaultClauses(unittest.TestCase):
    """Do: records that state SQLite's defaults (ABORT on conflict, an immediate foreign key, MATCH
    on REFERENCES, EXCLUDE NO OTHERS). Expect: each translates into every dialect, the clause gone."""

    def test_each_translates_everywhere(self):
        for sql, gone in [
            ("CREATE TABLE kab(k INTEGER PRIMARY KEY, v TEXT UNIQUE ON CONFLICT ABORT)", "ABORT"),
            ("CREATE TABLE kac(a INTEGER NOT NULL ON CONFLICT ABORT)", "ABORT"),
            ("UPDATE OR ABORT kab SET v = 'a' WHERE k = 3", "ABORT"),
            ("INSERT OR ABORT INTO kab VALUES (1, 'a')", "ABORT"),
            ("CREATE TABLE cd3(r INTEGER REFERENCES pd(k) NOT DEFERRABLE)", "DEFERRABLE"),
            ("CREATE TABLE cd5(r INTEGER, FOREIGN KEY (r) REFERENCES pd(k) NOT DEFERRABLE INITIALLY DEFERRED)", "DEFERRED"),
            ("CREATE TABLE fc5(a INTEGER, b INTEGER, FOREIGN KEY (a, b) REFERENCES fp(k, u) MATCH SIMPLE NOT DEFERRABLE INITIALLY IMMEDIATE)", "MATCH"),
            ("CREATE TABLE fc6(a INTEGER REFERENCES fp(k) ON DELETE CASCADE MATCH FULL)", "MATCH"),
            ("SELECT a, count(*) OVER (ORDER BY a ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING EXCLUDE NO OTHERS) FROM t1", "EXCLUDE"),
        ]:
            for d in ENGINES:
                with self.subTest(sql=sql, dialect=d):
                    out, why = dialects.translate(sql, d)
                    self.assertIsNone(why)
                    self.assertNotIn(gone, out.upper())

    def test_strings_names_and_the_match_operator_stay(self):
        """Do: the same words in a string, a quoted name and MATCH as an operator. Expect: untouched."""
        for sql in ["SELECT 'or abort', 'NOT DEFERRABLE' FROM t", 'SELECT "on conflict abort" FROM t', "SELECT a MATCH b FROM t"]:
            self.assertEqual(dialects.defaults_dropped(sql), sql)

    def test_other_exclusions_and_conflicts_stay_untranslatable(self):
        """Do: EXCLUDE CURRENT ROW for MySQL and ClickHouse, ON CONFLICT REPLACE anywhere. Expect:
        untranslatable, never silently dropped."""
        frame = "SELECT sum(a) OVER (ORDER BY a ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING EXCLUDE CURRENT ROW) FROM t1"
        for d in ("mysql", "clickhouse"):
            self.assertIsNotNone(dialects.translate(frame, d)[1])
        self.assertIsNotNone(dialects.translate("CREATE TABLE cf2(a TEXT UNIQUE ON CONFLICT REPLACE)", "postgres")[1])


class RefusedRecords(unittest.TestCase):
    """Do: a record SQLite refuses that sqlglot can't read, and one SQLite runs that sqlglot can't
    read. Expect: the first given as SQLite holds it, the second untranslatable."""

    def test_refused_record_is_given_as_written(self):
        sql = "SELECT (SELECT a WHERE a = 2 FROM t1)"
        for d in ENGINES:
            self.assertEqual(dialects.record_in(Record(sql, expect="error"), d), (sql, None))
            self.assertIsNotNone(dialects.record_in(Record(sql, kind="query", expect=None), d)[1])

    def test_refused_record_sqlglot_reads_is_translated(self):
        out, why = dialects.record_in(Record("UPDATE OR ABORT kab SET v = 'a' WHERE k = 3", expect="error"), "postgres")
        self.assertEqual((out, why), ("UPDATE kab SET v = 'a' WHERE k = 3", None))


class SQLiteReading(unittest.TestCase):
    """Do: forms SQLite takes that sqlglot's SQLite reader refused (n-ary iif and if, char() of
    nothing, a multi-word type name, ANY, STRICT, SAVEPOINT and RELEASE, REINDEX, a GROUPS frame
    with no ORDER BY, a block comment open at the end). Expect: each translates into every dialect,
    said as SQLite's documentation says it means."""

    def test_each_reads_and_translates_everywhere(self):
        for sql, said in [
            ("SELECT iif(0, 1, 0, 2, 3)", "CASE WHEN 0 THEN 1 WHEN 0 THEN 2 ELSE 3 END"),
            ("SELECT if(0, 1, 0, 2)", "CASE WHEN 0 THEN 1 WHEN 0 THEN 2 END"),
            ("SELECT char()", "SELECT ''"),
            ("RELEASE SAVEPOINT r3", "RELEASE SAVEPOINT r3"),
            ("RELEASE r3", "RELEASE SAVEPOINT r3"),
            ("SAVEPOINT r3", "SAVEPOINT r3"),
            ("REINDEX main.t2", "REINDEX main.t2"),
            ("SELECT 4 /* to the end", "SELECT 4 /* to the end */"),
            ("SELECT a, sum(a) OVER (GROUPS BETWEEN 1 PRECEDING AND CURRENT ROW) FROM t1", "GROUPS BETWEEN 1 PRECEDING AND CURRENT ROW"),
        ]:
            for d in ENGINES:
                with self.subTest(sql=sql, dialect=d):
                    out, why = dialects.translate(sql, d)
                    self.assertIsNone(why)
                    self.assertIn(said, out)

    def test_a_type_name_of_several_words_is_its_affinity(self):
        """FLOATING POINT is INTEGER (datatype3.html's own example), UNSIGNED BIG INT INTEGER,
        VARYING CHARACTER(255) TEXT, a lone ANY NUMERIC; constraints after it stay."""
        self.assertEqual(dialects.affinity("FLOATING POINT"), "INTEGER")
        self.assertEqual(dialects.affinity("VARYING CHARACTER"), "TEXT")
        self.assertEqual(dialects.affinity("DOUBLE PRECISION"), "REAL")
        self.assertEqual(dialects.affinity("ANY"), "NUMERIC")
        out, why = dialects.translate("CREATE TABLE r1(x FLOATING POINT, y UNSIGNED BIG INT NOT NULL, z VARYING CHARACTER(255) DEFAULT 'a', a ANY)", "postgres")
        self.assertIsNone(why)
        self.assertEqual(out, "CREATE TABLE r1 (x INT, y INT NOT NULL, z TEXT DEFAULT 'a', a DECIMAL)")

    def test_strict_table_is_typed_and_its_any_by_its_values(self):
        """An ANY column keeps values as written, as a column without a type does (UntypedColumns)."""
        file = ["CREATE TABLE st(i INTEGER, a ANY) STRICT", "INSERT INTO st VALUES (1, 'a')"]
        for d in ENGINES:
            out, why = file_in(file, d)[0]
            self.assertIsNone(why)
            self.assertNotIn("STRICT", out.upper())
        self.assertEqual(file_in(file, "postgres")[0][0], "CREATE TABLE st (i INT, a TEXT)")

    def test_conflict_resolutions_are_not_expressible_with_the_reason(self):
        for sql, clause in [("UPDATE OR IGNORE cf SET u = 'a' WHERE k = 2", "UPDATE OR IGNORE"), ("UPDATE OR REPLACE cf SET u = 'zz'", "UPDATE OR REPLACE"), ("CREATE TABLE cf2(a TEXT UNIQUE ON CONFLICT REPLACE)", "ON CONFLICT REPLACE")]:
            for d in ENGINES:
                out, why = dialects.translate(sql, d)
                self.assertIsNone(out)
                self.assertIn(f"not expressible in {d}", why)
                self.assertIn(clause, why)
        self.assertIsNone(dialects.translate("INSERT INTO kv VALUES (1) ON CONFLICT (k) DO NOTHING", "postgres")[1])

    def test_comment_closing_reads_past_strings_and_line_comments(self):
        self.assertEqual(dialects.comment_closed("SELECT '/*' -- /* x"), "SELECT '/*' -- /* x")
        self.assertEqual(dialects.comment_closed("SELECT 1 /* a */ /* b"), "SELECT 1 /* a */ /* b*/")


class FrameExclusions(unittest.TestCase):
    """Do: EXCLUDE CURRENT ROW, GROUP and TIES for MySQL and ClickHouse, which have no EXCLUDE.
    Expect: said exactly where count() arithmetic or an empty exclusion says it (checked against
    DuckDB's EXCLUDE on random rows, all NULL placements, as the lane's run reports), else not
    expressible, with the reason; DuckDB and PostgreSQL keep EXCLUDE."""

    def test_exact_rewrites(self):
        for sql, said in [
            ("SELECT count(b) OVER (ORDER BY a ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING EXCLUDE CURRENT ROW) FROM t1", "(COUNT(b) OVER (ORDER BY a ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING) - CASE WHEN b IS NULL THEN 0 ELSE 1 END)"),
            ("SELECT count(*) OVER (PARTITION BY c ORDER BY a RANGE BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW EXCLUDE GROUP) FROM t1", "(COUNT(*) OVER (PARTITION BY c ORDER BY a RANGE BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) - COUNT(*) OVER (PARTITION BY c, a))"),
            ("SELECT count(*) OVER (ORDER BY a RANGE BETWEEN 1 PRECEDING AND 1 FOLLOWING EXCLUDE TIES) FROM t1", "(COUNT(*) OVER (ORDER BY a RANGE BETWEEN 1 PRECEDING AND 1 FOLLOWING) - COUNT(*) OVER (PARTITION BY a) + 1)"),
            ("SELECT first_value(b) OVER (ORDER BY a ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING EXCLUDE CURRENT ROW) FROM t1", "FIRST_VALUE(b) OVER (ORDER BY a ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING)"),
        ]:
            out, why = dialects.translate(sql, "mysql")
            self.assertIsNone(why)
            self.assertIn(said, out)
            self.assertNotIn("EXCLUDE", dialects.translate(sql, "clickhouse")[0])
            for d in ("duckdb", "postgres"):
                self.assertIn("EXCLUDE", dialects.translate(sql, d)[0])

    def test_no_exact_rewrite_is_not_expressible(self):
        for sql in [
            "SELECT sum(b) OVER (ORDER BY a ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING EXCLUDE CURRENT ROW) FROM t1",
            "SELECT count(*) OVER (ORDER BY a ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING EXCLUDE GROUP) FROM t1",
            "SELECT last_value(b) OVER (ORDER BY a ROWS BETWEEN UNBOUNDED PRECEDING AND 1 PRECEDING EXCLUDE GROUP) FROM t1",
        ]:
            for d in ("mysql", "clickhouse"):
                out, why = dialects.translate(sql, d)
                self.assertIsNone(out)
                self.assertIn(f"not expressible in {d}: a window frame's EXCLUDE", why)


class ParseGaps(unittest.TestCase):
    """Do: what sqlglot's SQLite reader refused in the gate's pool (lane corpus-dialects-5): VALUES
    after WITH, a table PRIMARY KEY with ASC, COLLATE or AUTOINCREMENT, table constraints without a
    comma, several upsert clauses, and MySQL's NATURAL FULL JOIN. Expect: each said as SQLite's
    documentation says it means, else not expressible with the reason."""

    def test_values_after_with_is_its_selects(self):
        sql = "WITH x(a) AS (SELECT 1) VALUES (1, 'a'), (2, 'b')"
        for d in ENGINES:
            out, why = dialects.translate(sql, d)
            self.assertIsNone(why, d)
            self.assertIn("UNION ALL", out)
            self.assertIn("column1", out)

    def test_table_constraints_without_a_comma(self):
        for d in ENGINES:
            out, why = dialects.translate("CREATE TABLE ck(a INTEGER, CHECK (a > 0) CHECK (a < 9))", d)
            self.assertIsNone(why, d)
            self.assertEqual(out.upper().count("CHECK"), 2, (d, out))

    def test_a_primary_keys_order_is_left_out_and_autoincrement_moves_to_its_column(self):
        out, why = dialects.translate("CREATE TABLE pk(x INTEGER, y TEXT, PRIMARY KEY (x DESC))", "postgres")
        self.assertIsNone(why)
        self.assertNotIn("DESC", out)
        out, why = dialects.translate("CREATE TABLE tc6(a INTEGER, b TEXT, PRIMARY KEY (a AUTOINCREMENT))", "mysql")
        self.assertIsNone(why)
        self.assertIn("PRIMARY KEY AUTO_INCREMENT", out)
        out, why = dialects.translate("CREATE TABLE tc6(a INTEGER, b TEXT, PRIMARY KEY (a AUTOINCREMENT))", "duckdb")
        self.assertIsNone(out)
        self.assertIn("not expressible in duckdb: AUTOINCREMENT", why)

    def test_a_primary_key_collation_other_than_the_columns_is_not_expressible(self):
        for d in ENGINES:
            out, why = dialects.translate("CREATE TABLE ixt(k TEXT, v TEXT, PRIMARY KEY (k COLLATE NOCASE DESC))", d)
            self.assertIsNone(out)
            self.assertIn(f"not expressible in {d}: a PRIMARY KEY column's own collation", why)
            self.assertIsNone(dialects.translate("CREATE TABLE ixb(k TEXT, v TEXT, PRIMARY KEY (k COLLATE BINARY))", d)[1], d)

    def test_several_upsert_clauses_are_not_expressible(self):
        sql = "INSERT INTO kv(k, v, u) VALUES ('b', 7, 'ub') ON CONFLICT (k) DO UPDATE SET v = -1 ON CONFLICT (u) DO NOTHING"
        for d in ENGINES:
            out, why = dialects.translate(sql, d)
            self.assertIsNone(out)
            self.assertIn("several upsert clauses", why)
        self.assertEqual(dialects.upserts("SELECT 'ON CONFLICT DO'; INSERT INTO kv VALUES (1) ON CONFLICT DO NOTHING"), 1)

    def test_natural_full_join_on_mysql(self):
        out, why = dialects.translate("SELECT a FROM t1 NATURAL FULL JOIN t2", "mysql")
        self.assertIsNone(out)
        self.assertIn("not expressible in mysql: a NATURAL FULL JOIN", why)
        self.assertIsNone(dialects.translate("SELECT a FROM t1 NATURAL FULL JOIN t2", "postgres")[1])


class ExclusionAround(unittest.TestCase):
    """Do: min, max, first_value and last_value with EXCLUDE CURRENT ROW on a ROWS frame holding the
    row, for MySQL and ClickHouse. Expect: the two parts of the frame around the row, no EXCLUDE
    (checked against DuckDB's EXCLUDE on random rows by the lane's run); sum and nth_value stay not
    expressible."""

    def test_around_the_current_row(self):
        for f in ("min(b)", "max(b)", "first_value(b)", "last_value(b)"):
            sql = f"SELECT {f} OVER (ORDER BY a ROWS BETWEEN 2 PRECEDING AND 1 FOLLOWING EXCLUDE CURRENT ROW) FROM t1"
            for d in ("mysql", "clickhouse"):
                out, why = dialects.translate(sql, d)
                self.assertIsNone(why, (f, d))
                self.assertNotIn("EXCLUDE", out)
                self.assertIn("ROWS BETWEEN 2 PRECEDING AND 1 PRECEDING", out)
                self.assertIn("ROWS BETWEEN 1 FOLLOWING AND 1 FOLLOWING", out)
        out, _ = dialects.translate("SELECT max(b) OVER (ORDER BY a ROWS BETWEEN CURRENT ROW AND CURRENT ROW EXCLUDE CURRENT ROW) FROM t1", "mysql")
        self.assertEqual(out, "SELECT NULL FROM t1")
        for f in ("sum(b)", "nth_value(b, 2)"):
            self.assertIsNone(dialects.translate(f"SELECT {f} OVER (ORDER BY a ROWS BETWEEN 1 PRECEDING AND 1 FOLLOWING EXCLUDE CURRENT ROW) FROM t1", "mysql")[0], f)


class Autoincrement(unittest.TestCase):
    """Do: AUTOINCREMENT tables for PostgreSQL, plain, IF NOT EXISTS and TEMPORARY. Expect: no
    identity column; a count table and a BEFORE INSERT trigger after it (a fresh count unless IF
    NOT EXISTS); the TEMPORARY one not expressible."""

    def test_keys_given_as_sqlite_gives_them(self):
        out, why = dialects.translate("CREATE TABLE t(x INTEGER PRIMARY KEY AUTOINCREMENT, y TEXT)", "postgres")
        self.assertIsNone(why)
        self.assertNotIn("IDENTITY", out)
        self.assertIn("DROP TABLE IF EXISTS t_autoincrement", out)
        self.assertIn("BEFORE INSERT ON t FOR EACH ROW", out)
        self.assertIn("greatest((SELECT max(x) FROM t), (SELECT seq FROM t_autoincrement)) + 1", out)
        out, why = dialects.translate("CREATE TABLE IF NOT EXISTS t(x INTEGER PRIMARY KEY AUTOINCREMENT)", "postgres")
        self.assertIsNone(why)
        self.assertNotIn("DROP TABLE", out)
        self.assertIsNotNone(dialects.translate("CREATE TEMP TABLE t(x INTEGER PRIMARY KEY AUTOINCREMENT)", "postgres")[1])

    def test_other_dialects_unchanged(self):
        """Do: the same table for MySQL. Expect: its AUTO_INCREMENT, no trigger."""
        out, why = dialects.translate("CREATE TABLE t(x INTEGER PRIMARY KEY AUTOINCREMENT)", "mysql")
        self.assertIsNone(why)
        self.assertIn("AUTO_INCREMENT", out)
        self.assertNotIn("TRIGGER", out)


class Refusals(unittest.TestCase):
    """Do: a file whose statements SQLite refuses for four reasons (a UNIQUE key, a syntax error, a
    missing table, an existing one) and one it runs. Expect: SQLite's error kept for each refused
    record by its line, with its extended code where Python's sqlite3 gives it."""

    def test_each_refusal_kept_by_line(self):
        import tempfile

        text = "statement ok\nCREATE TABLE t(a INTEGER UNIQUE)\n\nstatement ok\nINSERT INTO t VALUES (1)\n\nstatement error\nINSERT INTO t VALUES (1)\n\nstatement error\nSELEC 1\n\nstatement error\nSELECT * FROM nope\n\nstatement error\nCREATE TABLE t(b)\n"
        with tempfile.TemporaryDirectory(dir=os.path.expanduser("~/.cache")) as pool:
            os.makedirs(os.path.join(pool, "src"))
            with open(os.path.join(pool, "src", "f.test"), "w", encoding="utf-8") as f:
                f.write(text)
            kept = dialects.refusals(pool, os.path.join("src", "f.test"))
        self.assertEqual([k["line"] for k in kept], [7, 10, 13, 16])
        self.assertIn("UNIQUE constraint failed", kept[0]["message"])
        self.assertIn("syntax error", kept[1]["message"])
        self.assertIn("no such table", kept[2]["message"])
        self.assertIn("already exists", kept[3]["message"])
        if sys.version_info >= (3, 11):
            self.assertEqual(kept[0]["code"], 2067)  # SQLITE_CONSTRAINT_UNIQUE (rescode.html)
            self.assertEqual(kept[1]["code"], 1)


class FileRecord(Record):
    def __init__(self, line, sql, kind="statement", expect="ok"):
        super().__init__(sql, kind, expect)
        self.line = line


def file_in(sqls, dialect):
    """Each of `sqls` (a file's records: SQL, or (SQL, "error") for one SQLite refuses) in `dialect`,
    each with its file's context, as dialects.main() writes them."""
    records = []
    for i, q in enumerate(sqls):
        sql, expect = q if isinstance(q, tuple) else (q, "ok")
        records.append(FileRecord(i, sql, "statement", expect))
    return dialects.records_in(records, dialect)


class UntypedColumns(unittest.TestCase):
    """Do: tables with columns without a type (no affinity: SQLite keeps each value as written),
    the file writing values of one storage class, of several, and none. Expect: the column declared
    the one class's type in every dialect; not expressible, with the reason, otherwise; never BLOB."""

    def test_one_class_gives_its_type(self):
        for d in ENGINES:
            with self.subTest(dialect=d):
                out = file_in(["CREATE TABLE t(a, b TEXT, c)", "INSERT INTO t VALUES (1, 'x', 'p'), (-2, 'y', NULL)", "INSERT INTO t(a, c) VALUES (3, 'q')", "UPDATE t SET c = 'r' || c WHERE a = 1"], d)
                sql, why = out[0]
                self.assertIsNone(why)
                self.assertNotIn("BLOB", sql.upper())
                self.assertNotIn("BYTEA", sql.upper())
                self.assertRegex(sql.upper(), r"\bA (BIGINT|INT64|NULLABLE\(INT64\))")
                self.assertRegex(sql.upper(), r"\bC (TEXT|VARCHAR\(255\)|STRING|NULLABLE\(STRING\))")

    def test_reals_strict_any_and_defaults(self):
        out = file_in(["CREATE TABLE t(a ANY, b DEFAULT 2.5) STRICT", "INSERT INTO t(a) VALUES (1.5)"], "postgres")
        self.assertEqual(out[0], ("CREATE TABLE t (a DOUBLE PRECISION, b DOUBLE PRECISION DEFAULT 2.5)", None))

    def test_mixed_classes_not_expressible(self):
        for d in ENGINES:
            with self.subTest(dialect=d):
                sql, why = file_in(["CREATE TABLE t(a)", "INSERT INTO t VALUES (1)", "INSERT INTO t VALUES ('one')"], d)[0]
                self.assertIsNone(sql)
                self.assertIn(f"not expressible in {d}", why)
                self.assertIn("several storage classes (INTEGER, TEXT)", why)
        sql, why = file_in(["CREATE TABLE t(a)", "INSERT INTO t VALUES (1), (2.5)"], "duckdb")[0]
        self.assertIn("(INTEGER, REAL)", why)

    def test_no_value_or_unknown_value_not_expressible(self):
        self.assertIn("no value but NULL", file_in(["CREATE TABLE t(a)", "INSERT INTO t VALUES (NULL)"], "duckdb")[0][1])
        self.assertIn("only running it shows", file_in(["CREATE TABLE t(a)", "INSERT INTO t VALUES (random_thing())"], "duckdb")[0][1])
        self.assertIn("weren't read", dialects.translate("CREATE TABLE t(a)", "duckdb")[1])

    def test_refused_writes_and_dropped_tables_dont_count(self):
        """Do: a write SQLite refuses, and a table dropped and made again. Expect: only what SQLite
        kept counts, each table by its own writes."""
        out = file_in(["CREATE TABLE t(a)", "INSERT INTO t VALUES (1)", ("INSERT INTO t VALUES ('no')", "error"), "DROP TABLE t", "CREATE TABLE t(a)", "INSERT INTO t VALUES ('text')"], "duckdb")
        self.assertIn("BIGINT", out[0][0])
        self.assertIn("TEXT", out[4][0])

    def test_insert_select_takes_the_source_columns_class(self):
        out = file_in(["CREATE TABLE s(x INTEGER)", "CREATE TABLE t(a)", "INSERT INTO t SELECT x * 2 FROM s"], "duckdb")
        self.assertIn("BIGINT", out[1][0])


class MissingTables(unittest.TestCase):
    """Do: a table whose CREATE a dialect can't say (its one column without a type holds values of
    several storage classes), then records that read it, write it, index it, view it, name it in a
    CTE's place, drop it and make it again. Expect: each record naming it (or a view of it)
    untranslatable with the CREATE's line and reason, where it would have failed on the engine for
    want of the table; one naming a CTE of the same name, or another table, translated; after the
    DROP, a translated CREATE makes it again; a CREATE SQLite refuses, or IF NOT EXISTS of a table
    already made, leaves it be."""

    def test_records_naming_it_are_untranslatable(self):
        for d in ENGINES:
            with self.subTest(dialect=d):
                out = file_in([
                    "CREATE TABLE t(e)", "INSERT INTO t VALUES (1), ('two')", "CREATE TABLE u(x INTEGER)",
                    "SELECT e FROM t", "SELECT x FROM u", "CREATE INDEX ti ON t(e)", "CREATE VIEW v AS SELECT e FROM t", "SELECT * FROM v",
                    "WITH t(a) AS (SELECT 1) SELECT a FROM t", "SELECT x FROM u WHERE x IN (SELECT e FROM t)",
                    "DROP TABLE t", "CREATE TABLE t(a INTEGER)", "SELECT a FROM t",
                ], d)
                self.assertIsNone(out[0][0])
                for i in (1, 3, 5, 6, 9, 10):
                    self.assertIsNone(out[i][0], i)
                    self.assertIn("names t, whose CREATE (line 0) is untranslatable there: not expressible", out[i][1])
                self.assertIn("names v, whose CREATE (line 6)", out[7][1])
                for i in (2, 4, 8, 11, 12):
                    self.assertIsNone(out[i][1], (i, out[i][1]))

    def test_refused_create_and_if_not_exists(self):
        out = file_in(["CREATE TABLE t(a INTEGER)", ("CREATE TABLE t(e)", "error"), "CREATE TABLE IF NOT EXISTS t(e)", "SELECT a FROM t"], "duckdb")
        self.assertIsNone(out[3][1])


class Collations(unittest.TestCase):
    """Do: SQLite's BINARY, NOCASE and RTRIM on a column and on an expression. Expect: DuckDB's
    "binary" and "nocase", PostgreSQL's "C", ClickHouse's bytes (the clause left out), MySQL's
    as before; RTRIM everywhere but MySQL, and NOCASE on PostgreSQL and ClickHouse, not
    expressible; on DuckDB, a comparison's operands both of the explicit collation SQLite picks (the
    left's first), and (when its Python module is here) BINARY orders and compares as SQLite does."""

    def test_each_dialect(self):
        sql = "CREATE TABLE r(b TEXT COLLATE BINARY, n TEXT COLLATE NOCASE)"
        self.assertEqual(dialects.translate(sql, "duckdb")[0], 'CREATE TABLE r (b TEXT COLLATE "binary", n TEXT COLLATE "nocase")')
        self.assertEqual(dialects.translate("SELECT b FROM r ORDER BY b COLLATE BINARY", "postgres")[0], 'SELECT b FROM r ORDER BY b COLLATE "C" NULLS FIRST')
        self.assertEqual(dialects.translate("CREATE TABLE r(b TEXT COLLATE BINARY)", "clickhouse")[0], "CREATE TABLE r (b Nullable(String))")
        self.assertIn("COLLATE utf8mb4_0900_as_ci", dialects.translate(sql, "mysql")[0])
        for d, word in (("duckdb", "RTRIM"), ("postgres", "RTRIM"), ("clickhouse", "RTRIM"), ("postgres", "NOCASE"), ("clickhouse", "NOCASE")):
            with self.subTest(dialect=d, collation=word):
                self.assertIn(f"not expressible in {d}: SQLite's collation {word}", dialects.translate(f"SELECT 'a' COLLATE {word} = 'a '", d)[1])
        self.assertIsNone(dialects.translate("SELECT 'a' COLLATE RTRIM = 'a '", "mysql")[1])
        self.assertEqual(dialects.translate("SELECT b COLLATE NOCASE = n COLLATE BINARY, b = 'a' COLLATE BINARY FROM r", "duckdb")[0], 'SELECT b COLLATE "nocase" = n COLLATE "nocase", b COLLATE "binary" = \'a\' COLLATE "binary" FROM r')

    def test_binary_orders_as_sqlite_on_duckdb(self):
        try:
            import duckdb
        except ImportError:
            self.skipTest("DuckDB's Python module isn't installed")
        import sqlite3

        s, d = sqlite3.connect(":memory:"), duckdb.connect()
        for q in ["CREATE TABLE r(b TEXT COLLATE NOCASE)", "INSERT INTO r VALUES ('a'), ('B'), ('A')"]:
            s.execute(q)
            d.execute(dialects.translate(q, "duckdb")[0])
        for q in ["SELECT b FROM r ORDER BY b COLLATE BINARY", "SELECT b = 'a' COLLATE BINARY FROM r ORDER BY b COLLATE BINARY"]:
            with self.subTest(sql=q):
                self.assertEqual([tuple(x) for x in d.execute(dialects.translate(q, "duckdb")[0]).fetchall()], [tuple(int(v) if isinstance(v, bool) else v for v in x) for x in s.execute(q).fetchall()])


class Total(unittest.TestCase):
    """Do: total() of integers, reals and NULL, as an aggregate, over no rows and over a window, and
    of text. Expect: on DuckDB (when its Python module is here) what SQLite gives (0.0 over no rows);
    of text left as it is."""

    def test_total_on_duckdb(self):
        try:
            import duckdb
        except ImportError:
            self.skipTest("DuckDB's Python module isn't installed")
        import sqlite3

        records = [FileRecord(0, "CREATE TABLE t(a INTEGER, c REAL)"), FileRecord(1, "INSERT INTO t VALUES (1, 1.5), (2, NULL), (NULL, 2.25)"),
                   FileRecord(2, "SELECT total(a), total(c), total(NULL) FROM t", "query", None), FileRecord(3, "SELECT total(a) FROM t WHERE a > 5", "query", None),
                   FileRecord(4, "SELECT a, total(c) OVER (ORDER BY a ROWS BETWEEN 1 PRECEDING AND CURRENT ROW) FROM t ORDER BY a", "query", None)]
        s, d = sqlite3.connect(":memory:"), duckdb.connect()
        for r, (sql, why) in zip(records, dialects.records_in(records, "duckdb")):
            self.assertIsNone(why)
            want = s.execute(r.sql).fetchall()
            got = d.execute(sql).fetchall()
            if r.kind == "query":
                with self.subTest(sql=r.sql):
                    self.assertEqual([tuple(x) for x in got], want)
        self.assertIn("TOTAL(B)", dialects.records_in([FileRecord(0, "CREATE TABLE u(b TEXT)"), FileRecord(1, "SELECT total(b) FROM u", "query", None)], "postgres")[1][0].upper())


class LeftOutColumns(unittest.TestCase):
    """Do: a table with typed columns and one without a type that holds values of several storage
    classes (the written files' shared t1), then inserts with and without a column list, an
    INSERT ... SELECT, queries that read the typed columns only, and ones that read the untyped
    one, `*`, NATURAL and USING joins, an UPDATE of it. Expect: the table made without that
    column; inserts without its values; the queries that don't read it translated; the rest
    untranslatable, naming the column and the CREATE's reason; on DuckDB (when its Python module
    is here) the translated queries give what SQLite gives."""

    FILE = ["CREATE TABLE t(a INTEGER, b TEXT, e)", "INSERT INTO t VALUES (1, 'one', 1), (2, 'two', 'x')", "INSERT INTO t(e, a) VALUES (x'07', 3)",
            "CREATE TABLE s(a INTEGER, e INTEGER)", "INSERT INTO s VALUES (4, 5)", "INSERT INTO t SELECT a, 'four', e FROM s",
            ("SELECT a, b FROM t ORDER BY a", "query"), ("SELECT count(*), sum(a) FROM t WHERE b IS NOT NULL", "query"),
            ("SELECT e FROM t", "query"), ("SELECT * FROM t", "query"), ("SELECT a FROM t NATURAL JOIN s", "query"), ("SELECT a FROM t JOIN s USING (a)", "query"),
            "UPDATE t SET e = 2 WHERE a = 1", "UPDATE t SET b = 'uno' WHERE a = 1", ("SELECT b FROM t WHERE a = 1", "query"),
            "CREATE TABLE w(x INTEGER, y TEXT, z INTEGER)", "INSERT INTO w SELECT a, b, a FROM t", ("SELECT x, y, z FROM w ORDER BY x", "query"),
            "CREATE TABLE v2(a INTEGER, f TEXT COLLATE RTRIM, g INTEGER)", ("SELECT t.a FROM t JOIN v2 ON t.a = v2.a", "query"), ("SELECT t.a FROM t JOIN v2 ON t.a = v2.a WHERE f = 'x'", "query"),
            ("SELECT count(*) FROM pragma_table_info('t')", "query"), ("PRAGMA table_info(t)", "query"), ("SELECT count(*) FROM pragma_table_info('w')", "query")]

    def records(self):
        out = []
        for i, q in enumerate(self.FILE):
            sql, kind = q if isinstance(q, tuple) else (q, "statement")
            out.append(FileRecord(i, sql, kind, "ok" if kind == "statement" else None))
        return out

    def test_left_out_where_not_read(self):
        for d in ENGINES:
            with self.subTest(dialect=d):
                out = dialects.records_in(self.records(), d)
                self.assertNotRegex(out[0][0], r"\be\b")
                self.assertIn("'one'", out[1][0])
                self.assertNotIn("7", out[2][0].replace("'\\x07'", "").replace("x'07'", ""))
                self.assertRegex(out[5][0], r"SELECT a, 'four' FROM s$")
                for i in (6, 7, 13, 14, 16, 17):
                    self.assertIsNone(out[i][1], (i, out[i][1]))
                self.assertRegex(out[16][0], r"SELECT a, b, a FROM t$")
                for i in (21, 22):
                    self.assertIn("inspects the columns of t, of which e is left out of its CREATE (line 0) there", out[i][1])
                self.assertIsNone(out[23][1])
                if d != "mysql":  # MySQL has RTRIM's like
                    self.assertIsNone(out[19][1], out[19][1])
                    self.assertIn("reads column f of v2", out[20][1])
                for i in (8, 9, 10, 11, 12):
                    self.assertIsNone(out[i][0], i)
                    self.assertIn("reads column e of t, left out of its CREATE (line 0) there: not expressible", out[i][1])

    def test_a_collation_the_dialect_has_none_like(self):
        records = [FileRecord(0, "CREATE TABLE t2(a INTEGER PRIMARY KEY, b TEXT COLLATE NOCASE, c INTEGER)"), FileRecord(1, "INSERT INTO t2 VALUES (1, 'x', 10)"),
                   FileRecord(2, "SELECT a, c FROM t2", "query", None), FileRecord(3, "SELECT b FROM t2", "query", None)]
        for d in ("postgres", "clickhouse"):
            with self.subTest(dialect=d):
                out = dialects.records_in(records, d)
                self.assertNotIn("b", out[0][0].replace("BIGINT", "").replace("Nullable", ""))
                self.assertIsNone(out[2][1])
                self.assertIn("reads column b of t2, left out of its CREATE (line 0) there: not expressible in " + d + ": SQLite's collation NOCASE", out[3][1])
        self.assertTrue(all(why is None for _, why in dialects.records_in(records, "duckdb")))

    def test_same_answers_as_sqlite_on_duckdb(self):
        try:
            import duckdb
        except ImportError:
            self.skipTest("DuckDB's Python module isn't installed")
        import sqlite3

        s, d = sqlite3.connect(":memory:"), duckdb.connect()
        for r, (sql, why) in zip(self.records(), dialects.records_in(self.records(), "duckdb")):
            want = s.execute(r.sql).fetchall()
            if why is None and r.kind != "query":
                d.execute(sql)
            elif why is None:
                with self.subTest(sql=r.sql):
                    got = d.execute(sql).fetchall()
                    self.assertEqual([tuple(x) for x in got], want)


class IntegerDivision(unittest.TestCase):
    """Do: `/` of two INTEGER operands (integer columns, typed by declaration or by their values,
    integer literals, count()), and of a REAL one. Expect: DuckDB's `//`, PostgreSQL's DIV with
    NULLIF on the divisor, MySQL's DIV for the first; `/` kept for the second and on ClickHouse."""

    FILE = ["CREATE TABLE t(a INTEGER, b, r REAL)", "INSERT INTO t VALUES (7, 2, 1.5)"]

    def last(self, sql, d):
        return file_in(self.FILE + [sql], d)[-1]

    def test_integers_divide_as_integers(self):
        sql = "SELECT a / b, 7 / 2, count(*) / 2, (a + 1) / -2 FROM t"
        self.assertEqual(self.last(sql, "duckdb"), ("SELECT a // b, 7 // 2, COUNT(*) // 2, (a + 1) // -2 FROM t", None))
        self.assertEqual(self.last(sql, "postgres"), ("SELECT DIV(a, NULLIF(b, 0)), DIV(7, NULLIF(2, 0)), DIV(COUNT(*), NULLIF(2, 0)), DIV((a + 1), NULLIF(-2, 0)) FROM t", None))
        self.assertEqual(self.last(sql, "mysql"), ("SELECT a DIV b, 7 DIV 2, COUNT(*) DIV 2, (a + 1) DIV -2 FROM t", None))
        self.assertNotIn("intDiv", self.last(sql, "clickhouse")[0])

    def test_reals_and_unknowns_keep_their_division(self):
        for sql in ["SELECT a / r FROM t", "SELECT 7.0 / 2 FROM t", "SELECT a / (SELECT 2) FROM t"]:
            with self.subTest(sql=sql):
                out, why = self.last(sql, "duckdb")
                self.assertIsNone(why)
                self.assertNotIn("//", out)

    def test_qualified_and_correlated_columns(self):
        sql = "SELECT x.a / y.b FROM t AS x JOIN t AS y ON 1 WHERE EXISTS (SELECT 1 FROM t AS z WHERE z.r = x.a / 2)"
        out, why = self.last(sql, "duckdb")
        self.assertIsNone(why)
        self.assertEqual(out.count("//"), 2)


class ClickHouseAutoincrement(unittest.TestCase):
    """Do: an AUTOINCREMENT key for ClickHouse, on its column and as the table's key. Expect: not
    expressible, with the reason (ClickHouse has no AUTO_INCREMENT)."""

    def test_not_expressible(self):
        for sql in ["CREATE TABLE t(x INTEGER PRIMARY KEY AUTOINCREMENT, y TEXT)", "CREATE TABLE t(x INTEGER, y TEXT, PRIMARY KEY(x AUTOINCREMENT))"]:
            with self.subTest(sql=sql):
                out, why = dialects.translate(sql, "clickhouse")
                self.assertIsNone(out)
                self.assertIn("not expressible in clickhouse: AUTOINCREMENT", why)


class HexIntegers(unittest.TestCase):
    """Do: SQLite's hexadecimal integer literals (0x10, 0X1f; lang_expr.html "Literal Values": "0x1234
    means the same as 4660"), beside a blob literal X'10'. Expect: each hex literal written as the
    decimal integer SQLite gives it (64-bit two's complement), the same value and an integer when
    SQLite itself runs the written text; the blob still a blob; one SQLite refuses ("hex literal too
    big") untranslatable."""

    CASES = ["0x10", "0X1f", "0x7FFFFFFFFFFFFFFF", "0x8000000000000000", "0xFFFFFFFFFFFFFFFF", "-0x10", "-0xFFFFFFFFFFFFFFFF", "0x00000000000000000001", "0x10 / 3", "(0xff)"]

    def test_written_as_the_integer_sqlite_gives(self):
        import sqlite3

        import sqlglot

        db = sqlite3.connect(":memory:")
        for case in self.CASES:
            want = db.execute(f"SELECT {case}, typeof({case})").fetchone()
            # The reading every dialect is written from, written back in SQLite's spelling for SQLite to run.
            said = sqlglot.parse_one(f"SELECT {case}", read=dialects.reader()).expressions[0].sql(dialect="sqlite")
            with self.subTest(case=case):
                self.assertEqual(db.execute(f"SELECT {said}, typeof({said})").fetchone(), want)
            for d in ENGINES:
                with self.subTest(case=case, dialect=d):
                    out, why = dialects.translate(f"SELECT {case}", d)
                    self.assertIsNone(why)
                    self.assertNotRegex(out.upper(), r"0X|UNHEX|BYTEA|X'")
        self.assertEqual(dialects.translate("SELECT 0x10, X'10'", "postgres"), ("SELECT 16, CAST('\\x10' AS BYTEA)", None))

    def test_too_big_is_untranslatable(self):
        for case in ["0x1FFFFFFFFFFFFFFFF", "-0x8000000000000000", "-(0x8000000000000000)"]:
            with self.subTest(case=case):
                out, why = dialects.translate(f"SELECT {case}", "postgres")
                self.assertIsNone(out)
                self.assertIn("hex literal too big", why)

    def test_untyped_column_takes_integer(self):
        """Do: a hex literal written to a column without a type. Expect: the column an integer one."""
        self.assertIn("BIGINT", file_in(["CREATE TABLE t(a)", "INSERT INTO t VALUES (0x10), (2)"], "duckdb")[0][0])


class ScalarExtremes(unittest.TestCase):
    """Do: SQLite's max() and min() of several arguments, with and without a NULL, nested, and
    beside the aggregate max(). Expect: each said with SQLite's NULL (NULL if any argument is,
    lang_corefunc.html#max_scalar), which SQLite gives the same answers to when it runs the CASE
    (written in its spelling); the aggregate untouched; one of random() not expressible."""

    CASES = ["max(1, NULL, 3)", "min(2, 7)", "max(min(5, 9), 2)", "min(NULL, 1)", "max(1, 3) / 2", "min(-0.25, -7)"]

    def test_null_if_any_argument_is(self):
        import sqlite3

        import sqlglot

        db = sqlite3.connect(":memory:")
        for case in self.CASES:
            for d in ENGINES:
                with self.subTest(case=case, dialect=d):
                    out, why = dialects.translate(f"SELECT {case}", d)
                    self.assertIsNone(why)
                    self.assertIn("IS NULL THEN NULL", out)
                    self.assertNotRegex(out.upper(), r"\b(MAX|MIN)\(")
            # What every dialect is written from, said in SQLite's spelling for SQLite to run.
            said = dialects.extremes(sqlglot.parse_one(f"SELECT {case}", read=dialects.reader()), "duckdb")
            with self.subTest(case=case):
                self.assertEqual(db.execute(said.sql(dialect="sqlite")).fetchone(), db.execute(f"SELECT {case}").fetchone())

    def test_aggregate_and_random(self):
        self.assertEqual(dialects.translate("SELECT max(a), min(a) OVER () FROM t", "postgres"), ("SELECT MAX(a), MIN(a) OVER () FROM t", None))
        out, why = dialects.translate("SELECT max(random(), 1)", "postgres")
        self.assertIsNone(out)
        self.assertIn("random()", why)


class Like(unittest.TestCase):
    """Do: SQLite's LIKE and like(): ASCII letters of another case, a backslash without an ESCAPE,
    an ESCAPE, NULL, and PRAGMA case_sensitive_like on. Expect: DuckDB (when its Python module is
    here) gives what SQLite gives for each; PostgreSQL and MySQL get ESCAPE '', ClickHouse its
    pattern's backslashes doubled; an ESCAPE that is a letter not expressible where case is folded."""

    CASES = ["'abc' LIKE 'A%'", "like('A%', 'abc')", "like('ab', 'AB')", "like('a\\%', 'a%')", "'a\\b' LIKE 'a\\b'", "'a%' LIKE 'a!%' ESCAPE '!'", "'ab' LIKE 'a!_' ESCAPE '!'", "NULL LIKE 'a'", "'É' LIKE 'é'", "'x' NOT LIKE 'X'"]

    def test_same_answers_as_sqlite_on_duckdb(self):
        try:
            import duckdb
        except ImportError:
            self.skipTest("DuckDB's Python module isn't installed")
        import sqlite3

        db = sqlite3.connect(":memory:")
        for case in self.CASES:
            with self.subTest(case=case):
                want = db.execute(f"SELECT {case}").fetchone()[0]
                out, why = dialects.translate(f"SELECT {case}", "duckdb")
                self.assertIsNone(why)
                got = duckdb.sql(out).fetchone()[0]
                self.assertEqual(None if got is None else int(got), want)

    def test_escapes_and_case(self):
        self.assertTrue(dialects.translate("SELECT a LIKE 'x%' FROM t", "postgres")[0].endswith("ESCAPE '' FROM t"))
        self.assertEqual(dialects.translate("SELECT a LIKE 'x%' FROM t", "mysql"), ("SELECT a LIKE 'x%' ESCAPE '' FROM t", None))
        self.assertIn("replaceAll(", dialects.translate("SELECT a LIKE 'x%' FROM t", "clickhouse")[0])
        self.assertNotIn("ESCAPE ''", dialects.translate("SELECT a LIKE 'x!%' ESCAPE '!' FROM t", "postgres")[0])
        for d in ("duckdb", "postgres", "clickhouse"):
            with self.subTest(dialect=d):
                self.assertIn("ASCII letter", dialects.translate("SELECT 'a%' LIKE 'aA%' ESCAPE 'A'", d)[1])

    def test_case_sensitive_like_pragma(self):
        """Do: LIKE before and after PRAGMA case_sensitive_like = on, then off. Expect: folded,
        not folded, folded."""
        out = file_in(["SELECT 'a' LIKE 'A'", "PRAGMA case_sensitive_like = on", "SELECT 'a' LIKE 'A'", "PRAGMA main.case_sensitive_like = 0", "SELECT 'a' LIKE 'A'"], "duckdb")
        self.assertEqual(["TRANSLATE" in (o[0] or "") for o in (out[0], out[2], out[4])], [True, False, True])


class FunctionsSaid(unittest.TestCase):
    """Do: likely(), unlikely(), likelihood(), upper() and lower() of letters beyond ASCII, round()
    to negative digits, char() of several code points and of NULL, format(), hex() of integers,
    text, a blob and NULL, zeroblob() of a count, a negative one and NULL, length() of an
    integer, a blob and text, hex() and length() of reals (SQLite's %!.15g text of them), typeof() and quote()
    of values of a known storage class, IS of values, date() with modifiers, CAST to an integer type of a real and of an integer past 32 bits. Expect: DuckDB (when its Python
    module is here) gives what SQLite gives, values compared as numbers; PostgreSQL's round() digits
    guarded; a likelihood() SQLite refuses left for the engine to refuse; hex() and length() of
    a REAL not expressible on PostgreSQL, MySQL and ClickHouse; typeof() and quote() of a known
    class said on every engine, of a REAL (quote) or an unknown class left as they are."""

    CASES = ["likely(1 > 0)", "unlikely(3) + 1", "likelihood(2, 0.5)", "upper('héllo')", "lower('ÉaB')", "round(2.5, -1)", "round(25, -2)", "char(72, 105)", "char(NULL)", "char(97)", "format('%d-%s', 7, 'x')", "format('%05.1f', 3.14159)", "hex(1)", "hex(-7)", "hex(NULL)", "hex('ab')", "hex(x'00ff')", "hex(1 + 2)", "zeroblob(3)", "zeroblob(-1)", "zeroblob(NULL)", "length(-7)", "length(x'00ff')", "length(1 + 22)", "length('é')", "hex(2.5)", "hex(100.0)", "hex(1e20)", "hex(-0.0)", "hex(9e999)", "length(1.0 / 3)", "length(1e-5)", "length(CAST(3 AS REAL))", "length(-9e999)", "length(123456789012345678.0)", "typeof(NULL)", "typeof(-7)", "typeof(2.5)", "typeof('abc')", "typeof(x'00')", "typeof(1 / 2.0)", "typeof(CAST(1 AS TEXT))", "quote(NULL)", "quote(-7)", "quote('it''s')", "quote(x'00ff')", "quote('')", "quote(x'')", "quote(1 + 2)", "2 IS 1", "NULL IS 1", "NULL IS NULL", "2 IS NOT 2", "'a' IS 'a'", "1 IS NOT NULL", "CAST(3.9 AS INTEGER)", "CAST(-2.5 AS INT)", "CAST(3000000000 AS INTEGER)", "CAST(2.5 AS TINYINT) + 1"]

    def test_same_answers_as_sqlite_on_duckdb(self):
        try:
            import duckdb
        except ImportError:
            self.skipTest("DuckDB's Python module isn't installed")
        import sqlite3

        db = sqlite3.connect(":memory:")
        for case in self.CASES:
            with self.subTest(case=case):
                want = db.execute(f"SELECT {case}").fetchone()[0]
                out, why = dialects.translate(f"SELECT {case}", "duckdb")
                self.assertIsNone(why)
                got = duckdb.sql(out).fetchone()[0]
                self.assertEqual(got if isinstance(want, (str, bytes)) else float(got), want)

    def test_other_dialects(self):
        for d in ENGINES:
            with self.subTest(dialect=d):
                out, why = dialects.translate("SELECT likely(a), likelihood(a, 0.25), round(a, -1) FROM t", d)
                self.assertIsNone(why)
                self.assertNotRegex(out.upper(), r"LIKEL|-1")
        self.assertIn("CASE WHEN b < 0 THEN 0 ELSE b END", dialects.translate("SELECT round(a, b) FROM t", "postgres")[0])
        self.assertIn("LIKELIHOOD(a, 1)", dialects.translate("SELECT likelihood(a, 1) FROM t", "postgres")[0])
        self.assertEqual(dialects.translate("SELECT upper(b) FROM t", "clickhouse"), ("SELECT UPPER(b) FROM t", None))
        self.assertEqual(dialects.translate("SELECT CAST(3.9 AS INTEGER), CAST(a AS INT) FROM t", "mysql"), ("SELECT CAST(TRUNCATE(3.9, 0) AS SIGNED), CAST(a AS SIGNED) FROM t", None))
        self.assertEqual(dialects.translate("SELECT zeroblob(2), zeroblob(2.9)", "postgres"), ("SELECT DECODE(REPEAT('00', COALESCE(2, 0)), 'hex'), ZEROBLOB(2.9)", None))
        self.assertEqual(dialects.translate("SELECT format('%d', 2), hex(1), hex(a)", "mysql"), ("SELECT PRINTF('%d', 2), COALESCE(HEX(CAST(1 AS CHAR)), ''), COALESCE(HEX(a), '')", None))
        records = [FileRecord(0, "CREATE TABLE t(a INTEGER, b TEXT, c BLOB)"), FileRecord(1, "SELECT typeof(a), quote(a), quote(b), quote(x'01'), quote(a + 0.5), typeof(abs(NULL)), typeof(c) FROM t", "query", None)]
        for d in ENGINES:
            with self.subTest(dialect=d):
                out = dialects.records_in(records, d)[1][0]
                self.assertIn("CASE WHEN a IS NULL THEN 'null' ELSE 'integer' END", out)
                self.assertEqual(out.upper().count("COALESCE("), 3)
                self.assertRegex(out.upper(), r"QUOTE\(A \+ 0\.5\)")
        for d in ENGINES:
            with self.subTest(dialect=d):
                self.assertIn(f"not expressible in {d}: date() with modifiers", dialects.translate("SELECT date('2024-01-31', '+1 month')", d)[1])
                self.assertIsNone(dialects.translate("SELECT date('2024-01-31')", d)[1])
        self.assertEqual(dialects.translate("SELECT a IS 1, a IS NOT 'x', a IS NULL FROM t", "postgres", dialects.contexts([FileRecord(0, "CREATE TABLE t(a INTEGER)")])[0])[0], "SELECT a IS NOT DISTINCT FROM 1, NOT a IS NOT DISTINCT FROM 'x', a IS NULL FROM t")
        self.assertIn("UPPER(ENCODE(CAST('\\x01' AS BYTEA), 'hex'))", dialects.records_in(records, "postgres")[1][0])
        for d in ("postgres", "mysql", "clickhouse"):
            for f in ("hex", "length") if d != "postgres" else ("length",):  # PostgreSQL has no hex()
                with self.subTest(dialect=d, function=f):
                    self.assertIn(f"not expressible in {d}: {f}() of a REAL", dialects.translate(f"SELECT {f}(2.5)", d)[1])


class Glob(unittest.TestCase):
    """Do: SQLite's GLOB and glob(): wildcards, sets with ranges and negation, a backslash (no escape
    in GLOB), regular expressions' own characters, a newline, a NULL pattern. Expect: DuckDB (when
    its Python module is here) gives what SQLite gives; each dialect a case-sensitive regular
    expression match; a pattern not a literal, an unclosed set or a backwards range not expressible."""

    CASES = ["'abc' GLOB 'a*'", "'abc' GLOB 'a?c'", "'bx' GLOB '[a-c]x'", "'' GLOB '*'", "'abc' GLOB 'A*'", "'a' GLOB NULL", "'bc' GLOB '[^a]*'", "'f.txt' GLOB '*.txt'", "'ab' GLOB '?'", "'a*' GLOB 'a[*]'", "glob('a\\*', 'a\\b')", "'a]' GLOB 'a[]]'", "'-' GLOB '[a-c-x]'", "'x' GLOB '[a-c-x]'", "'x' || char(10) || 'y' GLOB 'x?y'", "'axb' GLOB 'a.b'", "'abc' NOT GLOB 'a*'", "'é' GLOB '?'", "'(a|b)' GLOB '(*|?)'", "'a b' GLOB 'a b'"]

    def test_same_answers_as_sqlite_on_duckdb(self):
        try:
            import duckdb
        except ImportError:
            self.skipTest("DuckDB's Python module isn't installed")
        import sqlite3

        db = sqlite3.connect(":memory:")
        for case in self.CASES:
            with self.subTest(case=case):
                want = db.execute(f"SELECT {case}").fetchone()[0]
                out, why = dialects.translate(f"SELECT {case}", "duckdb")
                self.assertIsNone(why)
                got = duckdb.sql(out).fetchone()[0]
                self.assertEqual(None if got is None else int(got), want)

    def test_each_dialect(self):
        sql = "SELECT a GLOB '[^a]*.t?t' FROM t"
        self.assertEqual(dialects.translate(sql, "postgres"), ("SELECT a ~ '^[^a].*\\.t.t$' FROM t", None))
        self.assertEqual(dialects.translate(sql, "mysql"), ("SELECT REGEXP_LIKE(a, '(?s)^[^a].*\\\\.t.t\\\\z', 'c') FROM t", None))
        self.assertEqual(dialects.translate(sql, "clickhouse"), ("SELECT match(a, '(?s)^[^a].*\\\\.t.t$') FROM t", None))
        for sql, why in [("SELECT a GLOB b FROM t", "isn't a string literal"), ("SELECT 'a' GLOB 'a[b'", "not closed"), ("SELECT 'a' GLOB '[z-a]'", "before its start")]:
            for d in ENGINES:
                with self.subTest(sql=sql, dialect=d):
                    self.assertIn(why, dialects.translate(sql, d)[1])


if __name__ == "__main__":
    unittest.main()
