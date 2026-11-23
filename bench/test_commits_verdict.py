"""commits_verdict.py on records made up as bench/commits.sh writes them (goal "Many clients write
through SQL at once, safely"; T-T1.4.1). Run: python3 -m unittest bench/test_commits_verdict.py"""
import contextlib
import io
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import commits_verdict  # noqa: E402

PLATFORM = {"host": "adacachy2", "kernel": "Linux 6.17 x86_64", "fs": "btrfs", "device": "/dev/nvme0n1p2", "dir": "/d"}
DRIVER = "amphora-bench --powercut bench: one process, a thread and a connection a writer, the same commit loop and clock for every engine"


def record(system, writers, rate, p99_us=1000.0, p999_us=2000.0, platform=PLATFORM, driver=DRIVER):
    return {"system": system, "writers": writers, "commits_per_s": rate, "p50_us": 500.0, "p99_us": p99_us,
            "p999_us": p999_us, "refused": 0, "platform": platform,
            "client": {"driver": driver, "cores_busy": 2.5, "cores": 32}}


def ladder(pager32=25000.0, versioned32=30000.0, **kw):
    """Every system at 1 and 32 writers; Amphora's storages at the given rates at 32."""
    return [record("pager", 1, 2000.0), record("versioned", 1, 2000.0), record("sqlite", 1, 2000.0), record("postgres", 1, 1500.0),
            record("pager", 32, pager32, **kw), record("versioned", 32, versioned32, **kw),
            record("sqlite", 32, 10000.0), record("postgres", 32, 15000.0)]


class Verdict(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()

    def tearDown(self):
        self.tmp.cleanup()

    def verdict(self, records):
        path = os.path.join(self.tmp.name, "records.jsonl")
        with open(path, "w", encoding="utf-8") as f:
            for r in records:
                f.write(json.dumps(r) + "\n")
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            rc = commits_verdict.main([path])
        return rc, out.getvalue()

    def test_bars_met_and_the_competitors_compared_at_32(self):
        """Do: versioned at 30,000/s and pager at 25,000/s from 32 writers (2,000/s from one), p99 1 ms,
        p99.9 2 ms; SQLite 10,000/s, PostgreSQL 15,000/s. Expect: exit 0, the platform and driver named,
        versioned 3.00x SQLite's and 2.00x PostgreSQL's, every bar MEETS for both storages."""
        rc, out = self.verdict(ladder())
        self.assertEqual(rc, 0, out)
        self.assertIn(f"client: {DRIVER}", out)
        self.assertIn('"host": "adacachy2"', out)
        self.assertIn("vs 32: versioned: 3.00x SQLite's, 2.00x PostgreSQL's commits a second", out)
        for ac in ("AC-T1.3.2", "AC-T1.3.3", "AC-T1.3.4", "AC-T1.3.5"):
            for s in ("pager", "versioned"):
                self.assertRegex(out, rf"{ac} [^:]*: MEETS {s}:")

    def test_slow_commits_miss(self):
        """Do: versioned at 15,000/s from 32 writers, p99 6 ms, p99.9 20 ms. Expect: AC-T1.3.2 to 1.3.5
        MISS for versioned (7.50x one writer's), exit 0: a missed bar is a result."""
        recs = ladder(versioned32=15000.0)
        recs[5] = record("versioned", 32, 15000.0, p99_us=6000.0, p999_us=20000.0)
        rc, out = self.verdict(recs)
        self.assertEqual(rc, 0, out)
        self.assertIn("AC-T1.3.2 commits a second: MISSES versioned: 15000/s", out)
        self.assertIn("AC-T1.3.3 p99: MISSES versioned: p99 6.00 ms", out)
        self.assertIn("AC-T1.3.4 p99.9: MISSES versioned: p99.9 20.00 ms", out)
        self.assertIn("AC-T1.3.5 32 writers over 1: MISSES versioned: 7.50x one writer's", out)
        self.assertIn("AC-T1.3.2 commits a second: MEETS pager", out)

    def test_another_platform_is_refused(self):
        """Do: PostgreSQL's record at 32 names another host. Expect: exit 2, refused, nothing compared."""
        recs = ladder()
        recs[7] = record("postgres", 32, 15000.0, platform={**PLATFORM, "host": "jupiter"})
        rc, out = self.verdict(recs)
        self.assertEqual(rc, 2, out)
        self.assertIn("refused: the records name 2 platforms and 1 drivers", out)
        self.assertNotIn("AC-T1.3.2", out)

    def test_another_driver_is_refused(self):
        """Do: SQLite's record at 32 names another client. Expect: exit 2, refused."""
        recs = ladder()
        recs[6] = record("sqlite", 32, 10000.0, driver="sqlite3 shell")
        rc, out = self.verdict(recs)
        self.assertEqual(rc, 2, out)
        self.assertIn("1 platforms and 2 drivers", out)

    def test_a_missing_competitor_or_count_is_unsettled(self):
        """Do: no PostgreSQL record at 32, and no versioned record at 1. Expect: PostgreSQL's comparison
        UNSETTLED at 32, versioned's AC-T1.3.5 UNSETTLED (no record at 1 writer), its 1.3.2 still judged."""
        recs = [r for r in ladder() if not (r["system"] == "postgres" and r["writers"] == 32)
                and not (r["system"] == "versioned" and r["writers"] == 1)]
        rc, out = self.verdict(recs)
        self.assertEqual(rc, 0, out)
        self.assertIn("vs 32: versioned: 3.00x SQLite's, PostgreSQL's UNSETTLED (no record) commits a second", out)
        self.assertIn("AC-T1.3.5 32 writers over 1: UNSETTLED versioned: no record at 1 writer", out)
        self.assertIn("AC-T1.3.2 commits a second: MEETS versioned", out)

    def test_no_records_is_refused(self):
        """Do: an empty records file. Expect: exit 2."""
        rc, out = self.verdict([])
        self.assertEqual(rc, 2, out)


if __name__ == "__main__":
    unittest.main()
