#!/usr/bin/env python3
"""loops.py (PL.6, AC-PL.6.1) on step 3's records and on plans written for it: step 3's JOB pass
counts 30 of 113 plans and its TPC-H passes none; a scan beneath a scan counts, a scan or hash join
beneath a lookup by rowid doesn't, a hash join beneath a scan does, a hash join's build table read
once after its probe doesn't, and each subquery's block counts apart from its parent's.

usage: python3 -I bench/planner/test_loops.py
"""

import io
import os
import sys
import unittest
from contextlib import redirect_stdout

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import loops  # noqa: E402

RECORDS = os.path.join(HERE, "..", "records")


def node(id, detail, op, parent=None):
    return {"id": id, "parent": parent, "detail": detail, "op": op}


SCAN = {"type": "scan", "source": "table"}
ROWID = {"type": "search", "search_kind": "rowid_eq"}
SEEK = {"type": "search", "search_kind": "seek"}
HASH = {"type": "hash_join"}


class StepThree(unittest.TestCase):
    def counted(self, name):
        with redirect_stdout(io.StringIO()):
            return loops.count(os.path.join(RECORDS, name))

    def test_job_has_30_of_113(self):
        self.assertEqual(self.counted("2026-10-26-jupiter-planner-job-a.jsonl"), (30, 113))

    def test_tpch_has_none(self):
        self.assertEqual(self.counted("2026-10-26-jupiter-planner-tpch-sf1-a.jsonl"), (0, 22))
        self.assertEqual(self.counted("2026-10-26-jupiter-planner-tpch-sf10-a.jsonl"), (0, 22))


class Plans(unittest.TestCase):
    def found(self, *nodes):
        return [detail for detail, _ in loops.whole_reads_in_loops({"nodes": list(nodes)})]

    def test_a_scan_beneath_a_scan_counts(self):
        self.assertEqual(self.found(node(1, "SCAN p", SCAN), node(2, "SCAN q", SCAN)), ["SCAN q"])

    def test_a_scan_beneath_a_seek_counts(self):
        self.assertEqual(self.found(node(1, "SEARCH p", SEEK), node(2, "SCAN q", SCAN)), ["SCAN q"])

    def test_a_scan_or_hash_join_beneath_a_rowid_lookup_does_not(self):
        self.assertEqual(self.found(node(1, "SEARCH p", ROWID), node(2, "SCAN q", SCAN)), [])
        self.assertEqual(self.found(node(1, "SEARCH p", ROWID), node(2, "HASH JOIN q", HASH)), [])

    def test_a_hash_join_beneath_a_scan_counts(self):
        self.assertEqual(self.found(node(1, "SCAN p", SCAN), node(2, "HASH JOIN q", HASH)), ["HASH JOIN q"])

    def test_a_first_read_and_a_seek_do_not(self):
        self.assertEqual(self.found(node(1, "SCAN p", SCAN), node(2, "SEARCH q", SEEK)), [])
        self.assertEqual(self.found(node(1, "HASH JOIN q", HASH)), [])

    def test_a_hash_joins_build_read_does_not_count(self):
        probe = {"type": "hash_join", "table": "q", "build": "p"}
        build = {"type": "scan", "source": "table", "table": "p"}
        self.assertEqual(self.found(node(1, "HASH JOIN q", probe), node(2, "SCAN p", build)), [])
        aliased = {"type": "hash_join", "table": "s", "build": "r2"}
        build = {"type": "scan", "source": "table", "table": "r", "alias": "r2"}
        self.assertEqual(self.found(node(1, "HASH JOIN s", aliased), node(2, "SCAN r AS r2", build)), [])

    def test_a_scan_that_is_not_the_build_still_counts(self):
        probe = {"type": "hash_join", "table": "s", "build": "r"}
        build = {"type": "scan", "source": "table", "table": "r"}
        other = {"type": "scan", "source": "table", "table": "u"}
        self.assertEqual(self.found(node(1, "HASH JOIN s", probe), node(2, "SEARCH t", SEEK), node(3, "SCAN r", build), node(4, "SCAN u", other)), ["SCAN u"])

    def test_a_subquery_block_counts_apart(self):
        subquery = {"type": "scalar_subquery"}
        self.assertEqual(self.found(node(1, "SCAN p", SCAN), node(2, "SCALAR SUBQUERY 1", subquery), node(3, "SCAN q", SCAN, parent=2)), [])


if __name__ == "__main__":
    unittest.main()
