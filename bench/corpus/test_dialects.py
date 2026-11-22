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
            ("CREATE TABLE kac(a NOT NULL ON CONFLICT ABORT)", "ABORT"),
            ("UPDATE OR ABORT kab SET v = 'a' WHERE k = 3", "ABORT"),
            ("INSERT OR ABORT INTO kab VALUES (1, 'a')", "ABORT"),
            ("CREATE TABLE cd3(r INTEGER REFERENCES pd(k) NOT DEFERRABLE)", "DEFERRABLE"),
            ("CREATE TABLE cd5(r INTEGER, FOREIGN KEY (r) REFERENCES pd(k) NOT DEFERRABLE INITIALLY DEFERRED)", "DEFERRED"),
            ("CREATE TABLE fc5(a, b, FOREIGN KEY (a, b) REFERENCES fp(k, u) MATCH SIMPLE NOT DEFERRABLE INITIALLY IMMEDIATE)", "MATCH"),
            ("CREATE TABLE fc6(a REFERENCES fp(k) ON DELETE CASCADE MATCH FULL)", "MATCH"),
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

    def test_strict_table_is_typed_and_its_any_is_blob(self):
        for d in ENGINES:
            out, why = dialects.translate("CREATE TABLE st(i INTEGER, a ANY) STRICT", d)
            self.assertIsNone(why)
            self.assertNotIn("STRICT", out.upper())
        self.assertEqual(dialects.translate("CREATE TABLE st(i INTEGER, a ANY) STRICT", "postgres")[0], "CREATE TABLE st (i INT, a BYTEA)")

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


if __name__ == "__main__":
    unittest.main()
