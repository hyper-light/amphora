#!/usr/bin/env python3
"""adaptive.py (PL.13, AC-PL.13.1; lane replan-9): cold and warm times for every engine from a warm
pass's `query` records and cold.sh's `cold` records; the 20 slowest by the control's warm time; the
speed-up off/on each way, a query timed out on either engine left out of its geometric mean.

usage: python3 -I bench/planner/test_adaptive.py
"""

import json
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import adaptive  # noqa: E402


def warm(query, target, *ms):
    return {"record": "query", "bench": "job", "query": query, "target": target, "phase": "warm", "runs": [{"status": "done", "ms": m} for m in ms]}


def cold(query, target, ms):
    run = {"status": "done", "ms": ms} if ms is not None else {"status": "timed_out"}
    return {"record": "cold", "bench": "job", "query": query, "target": target, "run": run, "state": {"first_query_since_open": True}}


class ColdAndWarm(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)

    def write(self, name, records):
        path = os.path.join(self.dir.name, name)
        with open(path, "w") as f:
            f.write("\n".join(json.dumps(r) for r in [{"record": "run", "host": "h", "phase": name}] + records) + "\n")
        return path

    def test_each_engine_has_its_cold_and_warm_time_and_the_speedup_each_way(self):
        w = self.write("warm", [warm("1a", "amphora", 100, 120, 110), warm("1a", "amphora-adaptive", 50, 55, 60), warm("1a", "postgres", 80, 80, 80),
                                warm("2a", "amphora", 10, 10, 10), warm("2a", "amphora-adaptive", 10, 10, 10)])
        c = self.write("cold", [cold("1a", "amphora", 200), cold("1a", "amphora-adaptive", 160), cold("1a", "postgres", 300),
                                cold("2a", "amphora", 40), cold("2a", "amphora-adaptive", None)])
        rows, geo, n = adaptive.compare(*adaptive.read([w, c])[:2], top=20)
        self.assertEqual([r["query"] for r in rows], ["1a", "2a"])
        first = rows[0]
        self.assertEqual((first["amphora warm"], first["amphora-adaptive warm"], first["postgres warm"]), (110, 55, 80))
        self.assertEqual((first["amphora cold"], first["amphora-adaptive cold"], first["postgres cold"]), (200, 160, 300))
        self.assertEqual((first["speedup warm"], first["speedup cold"]), (2.0, 1.25))
        # 2a's adaptive cold run timed out: no cold speed-up, and the mean is 1a's alone.
        self.assertIsNone(rows[1]["speedup cold"])
        self.assertEqual(n, {"warm": 2, "cold": 1})
        self.assertAlmostEqual(geo["warm"], 2.0 ** 0.5)
        self.assertAlmostEqual(geo["cold"], 1.25)

    def test_the_slowest_are_the_controls_warm_and_a_query_without_cold_records_has_none(self):
        w = self.write("warm", [warm(q, "amphora", ms) for q, ms in (("1a", 5), ("2a", 50), ("3a", 20))])
        rows, geo, n = adaptive.compare(*adaptive.read([w])[:2], top=2)
        self.assertEqual([r["query"] for r in rows], ["2a", "3a"])
        self.assertIsNone(rows[0]["amphora cold"])
        self.assertEqual(n, {"warm": 0, "cold": 0})
        self.assertIsNone(geo["cold"])


if __name__ == "__main__":
    unittest.main()
