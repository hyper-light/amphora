#!/usr/bin/env python3
"""answers.py, the planner bench's whole-answer check, on answers written as amphora-bench writes
them. Each test serves kiln plan 01a188991617756c84d73f0e9e100330, step 1 (f), and its test: "The
whole-answer check given a result with one value changed, one with a row dropped, and one with its
rows reordered: passes when it flags the changed value and the dropped row, and flags the reorder
only where the query has ORDER BY".

usage: python3 -I bench/planner/test_answers.py [<answers.py>]
"""

import importlib.util
import os
import re
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = sys.argv.pop(1) if len(sys.argv) > 1 and sys.argv[1].endswith(".py") else os.path.join(HERE, "answers.py")
_spec = importlib.util.spec_from_file_location("answers", SCRIPT)
answers = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(answers)

ORDERED = "select k, v from t order by k"
UNORDERED = "select k, v from t"
THREE = "1\tx\n2\ty\n3\tz\n"


def rows(text):
    return answers.parse(text.encode(), "a test's answer")


def differs(sql, a, b):
    return answers.compare(rows(a), rows(b), answers.order_of(sql), ("a", "b"))


class Compare(unittest.TestCase):
    def test_a_changed_value_is_flagged(self):
        """Do: change one value of an answer. Expect: the row flagged, ordered or not."""
        for sql in (ORDERED, UNORDERED):
            self.assertIn("only in a: '2 | y'; only in b: '2 | q'", differs(sql, THREE, "1\tx\n2\tq\n3\tz\n"))
        self.assertTrue(differs(ORDERED, THREE, "1\tx\n2\tq\n3\tz\n").startswith("row 2: "))

    def test_a_dropped_row_is_flagged(self):
        """Do: drop a row. Expect: the counts and the row only one answer has, ordered or not."""
        for sql in (ORDERED, UNORDERED):
            self.assertEqual(differs(sql, THREE, "1\tx\n3\tz\n"), "3 rows against 2; only in a: '2 | y'")

    def test_reordered_rows_are_flagged_only_where_the_query_orders(self):
        """Do: swap two rows. Expect: flagged under ORDER BY, the same answer without it."""
        self.assertTrue(differs(ORDERED, THREE, "2\ty\n1\tx\n3\tz\n").startswith("row 1: "))
        self.assertIsNone(differs(UNORDERED, THREE, "2\ty\n1\tx\n3\tz\n"))
        self.assertIsNone(differs(UNORDERED, THREE, "3\tz\n2\ty\n1\tx\n"))

    def test_a_real_within_the_tolerance_agrees_and_one_beyond_it_differs(self):
        """Do: compare REALs against decimals as TPC-H's engines give them, then numbers apart by
        more than 1e-9, then integers past a double's precision. Expect: the first agree, the rest
        flagged."""
        # q01's sum_charge as Amphora and DuckDB gave it (the 2026-10-26 jupiter baseline, SF 1).
        self.assertIsNone(differs(UNORDERED, "55909065222.82769\n", "55909065222.827692\n"))
        self.assertIsNone(differs(UNORDERED, "0.30000000000000004\n", "0.3\n"))
        self.assertIsNone(differs(UNORDERED, "323.0\n", "323.00\n"))
        self.assertIsNotNone(differs(UNORDERED, "1.0\n", "1.000000002\n"))
        self.assertIsNotNone(differs(UNORDERED, "9007199254740993\n", "9007199254740992\n"))
        self.assertIsNotNone(differs(UNORDERED, "0.0\n", "1e-300\n"))

    def test_null_differs_from_empty_text(self):
        """Do: compare NULL against the empty text, and NULL against NULL. Expect: the first flagged."""
        self.assertIsNotNone(differs(UNORDERED, "\\N\n", "\n"))
        self.assertIsNotNone(differs(UNORDERED, "\\N\n", "\\\\N\n"))
        self.assertIsNone(differs(UNORDERED, "\\N\n", "\\N\n"))

    def test_rows_tied_on_the_order_by_keys_may_come_in_any_order(self):
        """Do: swap two rows tied on the ORDER BY's key, then move one out of its place. Expect: the
        swap the same answer, the move flagged."""
        tied = "1\tx\n1\ty\n2\tz\n"
        self.assertIsNone(differs(ORDERED, tied, "1\ty\n1\tx\n2\tz\n"))
        self.assertIsNotNone(differs(ORDERED, tied, "1\tx\n2\tz\n1\ty\n"))
        self.assertIsNotNone(differs(ORDERED, tied, "1\tx\n1\tq\n2\tz\n"))

    def test_a_real_key_within_the_tolerance_ties_whatever_keys_follow(self):
        """Do: order two rows as q03 does (revenue DESC, then date) where their revenues are equal
        as decimals and an ulp apart as REALs, so each engine puts a different one first. Expect: the
        same answer."""
        sql = "select l_orderkey, sum(x) as revenue, o_orderdate from t group by l_orderkey order by revenue desc, o_orderdate"
        real = "7\t100.00000000000001\t1995-01-02\n8\t100.0\t1995-01-01\n"
        self.assertIsNone(differs(sql, real, "8\t100.00\t1995-01-01\n7\t100.00\t1995-01-02\n"))
        self.assertIsNotNone(differs(sql, real, "8\t100.00\t1995-01-01\n7\t100.00\t1995-01-03\n"))

    def test_char_padding_is_not_a_difference(self):
        """Do: compare text against PostgreSQL's CHAR(n) padding of it, then with a leading blank.
        Expect: the padding the same answer, the leading blank flagged."""
        self.assertIsNone(differs(UNORDERED, "Brand#41\t28\n", "Brand#41  \t28\n"))
        self.assertIsNotNone(differs(UNORDERED, "Brand#41\t28\n", " Brand#41\t28\n"))

    def test_a_difference_tied_at_the_limit_is_flagged_with_why(self):
        """Do: give two answers to an ORDER BY ... LIMIT 2 whose second rows differ but tie on the
        key, then one whose first rows differ. Expect: both flagged, the tie at the LIMIT said."""
        sql = "select k, v from t order by k limit 2"
        self.assertIn("agree on the ORDER BY's keys at LIMIT 2", differs(sql, "1\tx\n2\ty\n", "1\tx\n2\tz\n"))
        self.assertNotIn("LIMIT", differs(sql, "1\tx\n2\ty\n", "1\tw\n2\ty\n"))


class OrderBy(unittest.TestCase):
    def test_tpch_orders_by_its_output_columns(self):
        """Do: read each TPC-H query's ORDER BY and LIMIT from bench/planner/tpch/queries. Expect:
        every key an output column, as the specification's queries name them."""
        expected = {
            "q01": [0, 1], "q02": [0, 2, 1, 3], "q03": [1, 2], "q04": [0], "q05": [1], "q06": [], "q07": [0, 1, 2], "q08": [0],
            "q09": [0, 1], "q10": [2], "q11": [1], "q12": [0], "q13": [1, 0], "q14": [], "q15": [0], "q16": [3, 0, 1, 2],
            "q17": [], "q18": [4, 3], "q19": [], "q20": [0], "q21": [1, 0], "q22": [0],
        }
        limits = {"q02": 100, "q03": 10, "q10": 20, "q18": 100, "q21": 100}
        for query, keys in expected.items():
            with open(os.path.join(HERE, "tpch", "queries", f"{query}.sql")) as f:
                self.assertEqual(answers.order_of(f.read()), answers.Order(keys, limits.get(query), None), query)

    def test_an_order_by_inside_parentheses_leaves_the_query_unordered(self):
        """Do: read queries whose only ORDER BY is in a subquery, a CTE or a window. Expect: none ordered."""
        for sql in ("select * from (select a from t order by a) s",
                    "with c as (select a from t order by a limit 3) select a from c",
                    "select a, row_number() over (order by a) from t"):
            self.assertEqual(answers.order_of(sql), answers.Order([], None, None), sql)

    def test_positions_expressions_and_modifiers_name_their_columns(self):
        """Do: order by a position, an expression and a name with NULLS, COLLATE and ASC after them.
        Expect: each the column it names."""
        sql = "SELECT a, SUM(b) AS total, \"Quoted\" FROM t GROUP BY a ORDER BY 2 DESC NULLS LAST, sum( b ), A COLLATE nocase ASC, \"quoted\";"
        self.assertEqual(answers.order_of(sql), answers.Order([1, 1, 0, 2], None, None))

    def test_a_key_not_in_the_output_stops_the_keys(self):
        """Do: order by a column the query doesn't output, between two it does. Expect: the key before
        it kept, it named."""
        self.assertEqual(answers.order_of("select a, b from t order by a, c, b"), answers.Order([0], None, "c"))

    def test_comments_and_strings_hide_what_they_hold(self):
        """Do: put ORDER BY and LIMIT in a string and in comments. Expect: the query unordered."""
        sql = "select a from t where s = 'x order by a' -- order by a\n/* order by a limit 1 */"
        self.assertEqual(answers.order_of(sql), answers.Order([], None, None))

    def test_the_caps_are_the_writers(self):
        """Do: read answers.rs's MAX_ROWS and MAX_BYTES. Expect: answers.py's bounds the same."""
        with open(os.path.join(HERE, "..", "src", "planner", "answers.rs")) as f:
            source = f.read()
        for constant, mine in (("MAX_ROWS: u64", answers.MAX_ROWS), ("MAX_BYTES: usize", answers.MAX_BYTES)):
            base, shift = re.search(rf"pub const {constant} = ([0-9]+) << ([0-9]+);", source).groups()
            self.assertEqual(int(base) << int(shift), mine, constant)


class Hostile(unittest.TestCase):
    def test_long_digits_read_as_text_or_nothing(self):
        """Do: read a field of 200,000 digits and a letter, a numeral past int()'s 4,300 digits, and
        a query whose ORDER BY position and LIMIT run to 5,000 digits. Expect: the field and the
        numeral text, no position or LIMIT taken, and no error."""
        field = "1" * 200_000 + "x"
        self.assertEqual(rows(field + "\n").values, [(field,)])
        self.assertEqual(rows("1" * 5000 + "\n").values, [("1" * 5000,)])
        self.assertEqual(answers.order_of(f"select a from t order by {'1' * 5000} limit {'9' * 5000}"), answers.Order([], None, "1" * 5000))


def write(root, files):
    for path, text in files.items():
        os.makedirs(os.path.dirname(os.path.join(root, path)), exist_ok=True)
        with open(os.path.join(root, path), "w") as f:
            f.write(text)


def run(*dirs):
    out = subprocess.run([sys.executable, "-I", SCRIPT, *dirs], capture_output=True, text=True)
    return out.returncode, out.stdout.splitlines(), out.stderr


Q01 = "select l_returnflag, l_linestatus, sum(l_quantity) as sum_qty, count(*) as count_order from lineitem group by l_returnflag, l_linestatus order by l_returnflag, l_linestatus"
Q01_ROWS = "A\tF\t37734107.0\t1478493\nN\tF\t991417.0\t38854\n"
Q05 = "select n_name, sum(l_extendedprice * (1 - l_discount)) as revenue from t group by n_name order by revenue desc"


class Directories(unittest.TestCase):
    def test_engines_are_compared_and_summed_up(self):
        """Do: check a pass's directory where DuckDB's count differs in q01's second row, PostgreSQL
        pads q05's names and DuckDB timed out on it, and JOB's 1a has one engine. Expect: DuckDB
        against the engines that agree, its time-out listed, two queries compared, one differing,
        exit 1."""
        with tempfile.TemporaryDirectory() as d:
            write(d, {
                "tpch-sf1/q01/query.sql": Q01,
                "tpch-sf1/q01/amphora.tsv": Q01_ROWS,
                "tpch-sf1/q01/sqlite.tsv": Q01_ROWS,
                "tpch-sf1/q01/duckdb.tsv": "A\tF\t37734107.00\t1478493\nN\tF\t991417.00\t38855\n",
                "tpch-sf1/q01/postgres.tsv": "A\tF\t37734107.00\t1478493\nN\tF\t991417.00\t38854\n",
                "tpch-sf1/q05/query.sql": Q05,
                "tpch-sf1/q05/amphora.tsv": "INDONESIA\t55502041.169700004\nVIETNAM\t55295086.99669999\n",
                "tpch-sf1/q05/postgres.tsv": "INDONESIA                \t55502041.1697\nVIETNAM                  \t55295086.9967\n",
                "tpch-sf1/q05/duckdb.tsv": "\\!timed_out the run passed its deadline\n",
                "job/1a/query.sql": "SELECT MIN(t.title) AS movie_title FROM title AS t;",
                "job/1a/amphora.tsv": "#1 Single\n",
            })
            code, lines, _ = run(d)
        self.assertEqual(lines, [
            "answers.py: tpch-sf1/q01: duckdb against amphora, sqlite, postgres: row 2: only in amphora: 'N | F | 991417.0 | 38854'; only in duckdb: 'N | F | 991417.00 | 38855'",
            "answers.py: tpch-sf1/q05 duckdb: no answer: timed_out the run passed its deadline",
            "answers.py: 2 queries compared, 1 differ",
        ])
        self.assertEqual(code, 1)

    def test_a_build_is_compared_with_its_parent(self):
        """Do: check a build whose Amphora dropped q01's second row against its parent's answers.
        Expect: the dropped row flagged against the parent, q05 the same, exit 1."""
        with tempfile.TemporaryDirectory() as build, tempfile.TemporaryDirectory() as parent:
            q05 = {"tpch-sf1/q05/query.sql": Q05, "tpch-sf1/q05/amphora.tsv": "INDONESIA\t55502041.169700004\n"}
            write(build, {"tpch-sf1/q01/query.sql": Q01, "tpch-sf1/q01/amphora.tsv": "A\tF\t37734107.0\t1478493\n", **q05})
            write(parent, {"tpch-sf1/q01/query.sql": Q01, "tpch-sf1/q01/amphora.tsv": Q01_ROWS, **q05})
            code, lines, _ = run(build, parent)
        self.assertEqual(lines, [
            "answers.py: tpch-sf1/q01: amphora against the parent: 2 rows against 1; only in amphora in the parent: 'N | F | 991417.0 | 38854'",
            "answers.py: 2 queries compared, 1 differ",
        ])
        self.assertEqual(code, 1)

    def test_answers_that_agree_exit_0(self):
        """Do: check a pass whose engines agree on every row. Expect: none differ, exit 0."""
        with tempfile.TemporaryDirectory() as d:
            write(d, {"tpch-sf1/q01/query.sql": Q01, "tpch-sf1/q01/amphora.tsv": Q01_ROWS, "tpch-sf1/q01/sqlite.tsv": Q01_ROWS})
            code, lines, _ = run(d)
        self.assertEqual((code, lines), (0, ["answers.py: 1 queries compared, 0 differ"]))

    def test_a_file_answers_rs_does_not_write_is_refused(self):
        """Do: check an answer holding an escape answers.rs never writes, then one whose last row has
        no newline. Expect: exit 2, the file and what is wrong with it named."""
        for text, why in (("a\\qb\n", "an escape answers.rs doesn't write: \\q"), ("a\nb", "its last row has no newline")):
            with tempfile.TemporaryDirectory() as d:
                write(d, {"job/1a/query.sql": "select 1", "job/1a/amphora.tsv": text, "job/1a/sqlite.tsv": "a\n"})
                code, lines, err = run(d)
            self.assertEqual(code, 2)
            self.assertIn(os.path.join("job", "1a", "amphora.tsv"), err)
            self.assertIn(why, err)


if __name__ == "__main__":
    unittest.main()
