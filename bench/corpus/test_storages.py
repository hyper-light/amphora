"""storages.py's verdicts on the corpus run on both storages (goal "Many clients write through SQL at
once, safely", T-T1.4.1): what it says of two runs made up here, as `amphora-corpus run --times`
writes them. Run: python3 -m unittest bench/corpus/test_storages.py"""
import contextlib
import io
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import storages  # noqa: E402

# A pool file of two queries and two writes, at the lines amphora-corpus numbers them by.
POOL = """statement ok
CREATE TABLE t(a INTEGER)

statement ok
INSERT INTO t VALUES (1)

statement ok
UPDATE t SET a = 2

query I nosort
SELECT a FROM t
----
2

query I nosort
SELECT count(*) FROM t
----
1
"""
LINES = {"create": 1, "insert": 4, "update": 7, "q1": 10, "q2": 15}
CLIENT, PLATFORM = "in process (the amphora crate, in memory)", "linux/x86_64"


def run_dir(root, name, ns, outcome=None, shown="", client=CLIENT, platform=PLATFORM):
    """A run's out.txt and times.jsonl: `ns` maps a record's name to its time; `outcome` to its
    outcome where not "agree"."""
    d = os.path.join(root, name)
    os.makedirs(d)
    with open(os.path.join(d, "times.jsonl"), "w", encoding="utf-8") as f:
        for rec, line in LINES.items():
            got = (outcome or {}).get(rec, "agree")
            f.write(json.dumps({"file": "a.test", "line": line, "outcome": got, "ns": ns[rec], "client": client, "platform": platform}) + "\n")
        f.write(json.dumps({"file": "a.test", "file_ns": 1, "required": True, "client": client, "platform": platform}) + "\n")
    with open(os.path.join(d, "out.txt"), "w", encoding="utf-8") as f:
        f.write(shown)
        f.write("amphora: 1 files in 0.1 s: ...\n")
    return d


def verdict(pool, pager, versioned):
    out = io.StringIO()
    argv = sys.argv
    sys.argv = ["storages.py", pool, pager, versioned]
    try:
        with contextlib.redirect_stdout(out):
            rc = storages.main()
    finally:
        sys.argv = argv
    return rc, out.getvalue()


class Storages(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = self.tmp.name
        self.pool = os.path.join(self.root, "pool")
        os.makedirs(self.pool)
        with open(os.path.join(self.pool, "a.test"), "w", encoding="utf-8") as f:
            f.write(POOL)

    def tearDown(self):
        self.tmp.cleanup()

    def test_same_answers_reads_as_fast_writes_three_times(self):
        """Do: both runs agree everywhere, versioned's reads as fast and its writes 4x as fast.
        Expect: exit 0, answers SAME, reads MEETS, writes MEETS, the one client and platform named."""
        pager = run_dir(self.root, "pager", {"create": 10, "insert": 400, "update": 800, "q1": 100, "q2": 200})
        versioned = run_dir(self.root, "versioned", {"create": 10, "insert": 100, "update": 200, "q1": 100, "q2": 200})
        rc, out = verdict(self.pool, pager, versioned)
        self.assertEqual(rc, 0, out)
        self.assertIn(f"client {CLIENT}, platform {PLATFORM}, both runs", out)
        self.assertIn("answers: SAME: 0 of 5", out)
        self.assertIn("reads: MEETS", out)
        self.assertIn("writes: MEETS", out)

    def test_a_record_answered_otherwise_differs(self):
        """Do: versioned disagrees with SQLite on one query, its answer on stdout.
        Expect: exit 1 and a `differ` line naming the record and both outcomes, with what it said."""
        ns = {"create": 10, "insert": 100, "update": 200, "q1": 100, "q2": 200}
        pager = run_dir(self.root, "pager", ns)
        shown = "a.test:10: SELECT a FROM t\n    SQLite answers 2\n    amphora answers 1\n"
        versioned = run_dir(self.root, "versioned", ns, outcome={"q1": "disagree"}, shown=shown)
        rc, out = verdict(self.pool, pager, versioned)
        self.assertEqual(rc, 1, out)
        self.assertIn("differ a.test:10: pager agree, versioned disagree", out)
        self.assertIn("amphora answers 1", out)
        self.assertIn("answers: DIFFER: 1 of 5", out)

    def test_writes_no_faster_miss(self):
        """Do: versioned's writes take what the pager's take. Expect: writes MISSES (the bar is 3x)."""
        pager = run_dir(self.root, "pager", {"create": 10, "insert": 100, "update": 210, "q1": 100, "q2": 200})
        versioned = run_dir(self.root, "versioned", {"create": 10, "insert": 105, "update": 200, "q1": 100, "q2": 200})
        rc, out = verdict(self.pool, pager, versioned)
        self.assertEqual(rc, 0, out)
        self.assertIn("writes: MISSES", out)

    def test_another_platform_is_refused(self):
        """Do: the versioned run names another platform. Expect: exit 2, refused, nothing compared."""
        ns = {"create": 10, "insert": 100, "update": 200, "q1": 100, "q2": 200}
        pager = run_dir(self.root, "pager", ns)
        versioned = run_dir(self.root, "versioned", ns, platform="macos/aarch64")
        rc, out = verdict(self.pool, pager, versioned)
        self.assertEqual(rc, 2, out)
        self.assertIn("refused", out)
        self.assertNotIn("answers:", out)


if __name__ == "__main__":
    unittest.main()
