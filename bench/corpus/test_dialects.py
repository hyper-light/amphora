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


if __name__ == "__main__":
    unittest.main()
