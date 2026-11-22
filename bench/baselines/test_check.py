#!/usr/bin/env python3
"""check.py on records as the harnesses write them (goal 63c98840).

usage: python3 -I bench/baselines/test_check.py
"""

import contextlib
import io
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import check  # noqa: E402


def query(name, *prepare_search, target="amphora", bench="job"):
    runs = [{"status": "done", "prepare_ms": p, "search_ms": s, "ms": p + 10, "rows": 1} for p, s in prepare_search]
    return {"record": "query", "bench": bench, "query": name, "target": target, "runs": runs}


def suite(target, items, *results):
    return {"host": "h", "cpu": "c", "cores": 4, "commit": "abc", "load_before": 0.5, "load_after": 0.6,
            "report": {"target": target, "items": items, "results": [{"op": op, "clients": 1, "kind": "Read", "errors": 0, "ms": {"p50": p50, "p99": p50 * 2}} for op, p50 in results]}}


def run(part, records):
    with tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False) as f:
        for r in records:
            f.write(json.dumps(r) + "\n")
    out = io.StringIO()
    try:
        with contextlib.redirect_stdout(out):
            code = check.main([part, f.name])
    finally:
        os.unlink(f.name)
    return code, out.getvalue()


class Plan(unittest.TestCase):
    def test_every_query_within_the_bound_passes(self):
        """Do: two queries, each's least planning time under 20 ms though a run is over. Expect: exit 0."""
        code, out = run("plan", [query("1a", (25.0, 1.0), (3.0, 1.0)), query("2a", (19.9, 5.0))])
        self.assertEqual(code, 0, out)
        self.assertIn("2 of 2 queries planned within 20 ms", out)

    def test_a_query_over_the_bound_fails_and_is_named(self):
        """Do: one query whose least planning time is 21 ms. Expect: exit 1, the query named."""
        code, out = run("plan", [query("1a", (2.0, 1.0)), query("29b", (21.0, 15.0), (22.0, 16.0))])
        self.assertEqual(code, 1)
        self.assertIn("over 20 ms: 29b 21.000 ms", out)

    def test_a_query_that_never_planned_fails(self):
        """Do: a query whose only run failed. Expect: exit 1, named as without a planning time."""
        failed = {"record": "query", "bench": "job", "query": "3a", "target": "amphora", "runs": [{"status": "failed", "error": "x"}]}
        code, out = run("plan", [query("1a", (2.0, 1.0)), failed])
        self.assertEqual(code, 1)
        self.assertIn("no planning time: 3a", out)

    def test_other_engines_are_left_out(self):
        """Do: a slow SQLite record beside Amphora's. Expect: only Amphora's counted."""
        code, out = run("plan", [query("1a", (2.0, 1.0)), query("1a", (90.0, 0.0), target="sqlite")])
        self.assertEqual(code, 0, out)
        self.assertIn("1 of 1", out)


class Search(unittest.TestCase):
    def test_search_and_its_share(self):
        """Do: two queries, searches 1 and 3 ms of plannings 2 and 6 ms. Expect: share 0.5."""
        code, out = run("search", [query("1a", (2.0, 1.0)), query("2a", (6.0, 3.0))])
        self.assertEqual(code, 0, out)
        self.assertIn("share of planning, over every query: 0.500", out)


class Leader(unittest.TestCase):
    def test_leader_is_the_fastest_other_row_store(self):
        """Do: point_pk on Amphora 0.004, SQLite 0.002, PostgreSQL 0.03, ClickHouse 0.001 ms.
        Expect: SQLite the leader (ClickHouse is no row store), Amphora at 2.000 of it."""
        recs = [suite("amphora", 100, ("point_pk", 0.004)), suite("sqlite", 100, ("point_pk", 0.002)),
                suite("postgres", 100, ("point_pk", 0.03)), suite("clickhouse", 100, ("point_pk", 0.001))]
        code, out = run("leader", recs)
        self.assertEqual(code, 0, out)
        self.assertIn("leader sqlite; Amphora's p50 over the leader's: 2.000", out)
        self.assertIn("commit abc", out)


class Scan(unittest.TestCase):
    def test_per_row_from_the_suite_and_tpch(self):
        """Do: table_count at p50 1 ms over 1000 rows, and TPC-H q01 at best 20 ms (10 + 10) over a
        lineitem of 1000 rows. Expect: 1000 and 20000 ns a row."""
        recs = [suite("amphora", 1000, ("table_count", 1.0), ("point_pk", 0.01)),
                {"record": "load", "target": "duckdb", "table": "lineitem", "rows": 1000},
                query("q01", (10.0, 0.0), target="duckdb", bench="tpch")]
        code, out = run("scan", recs)
        self.assertEqual(code, 0, out)
        self.assertIn("table_count amphora: 1000.00 ns a row", out)
        self.assertIn("tpch q01 duckdb: 20000.00 ns a row", out)


if __name__ == "__main__":
    unittest.main()
