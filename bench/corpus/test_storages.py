"""storages.py's verdicts on the corpus run on both storages (goal "Many clients write through SQL at
once, safely"; G7.5): what it says of two runs made up here, as `amphora-corpus run --times`
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
DISK_CLIENT = "in process (the amphora crate, on a file in /d: WAL, synchronous = FULL)"
SAME = {"create": 10, "insert": 100, "update": 200, "q1": 100, "q2": 200}


def run_dir(root, name, ns, outcome=None, shown="", client=CLIENT, platform=PLATFORM, missing=()):
    """A run's out.txt and times.jsonl: `ns` maps a record's name to its time; `outcome` to its
    outcome where not "agree"; the records named in `missing` have no line (the run stopped short)."""
    d = os.path.join(root, name)
    os.makedirs(d)
    with open(os.path.join(d, "times.jsonl"), "w", encoding="utf-8") as f:
        for rec, line in LINES.items():
            if rec in missing:
                continue
            got = (outcome or {}).get(rec, "agree")
            f.write(json.dumps({"file": "a.test", "line": line, "outcome": got, "ns": ns[rec], "client": client, "platform": platform}) + "\n")
        f.write(json.dumps({"file": "a.test", "file_ns": 1, "required": True, "client": client, "platform": platform}) + "\n")
    with open(os.path.join(d, "out.txt"), "w", encoding="utf-8") as f:
        f.write(shown)
        f.write("amphora: 1 files in 0.1 s: ...\n")
    return d


def verdict(pool, memory, disk):
    """storages.py's exit and output over a memory pair and a disk pair, each (pager, versioned)."""
    out = io.StringIO()
    argv = sys.argv
    sys.argv = ["storages.py", pool, *memory, *disk]
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

    def pair(self, where, pager_ns, versioned_ns, client=None, **kw):
        """A pair of runs under `where` ("memory" or "disk"), its client the in-memory or the disk one."""
        client = client or (CLIENT if where == "memory" else DISK_CLIENT)
        vkw = {k[2:]: v for k, v in kw.items() if k.startswith("v_")}
        both = {"outcome": kw["outcome_all"]} if "outcome_all" in kw else {}
        if "missing_all" in kw:
            both["missing"] = kw["missing_all"]
        vkw = {**both, **vkw}
        return (run_dir(self.root, f"{where}-pager", pager_ns, client=client, **both),
                run_dir(self.root, f"{where}-versioned", versioned_ns, client=client, **vkw))

    def test_same_answers_reads_as_fast_writes_three_times(self):
        """Do: every run agrees everywhere; on the disk versioned's reads are as fast and its writes
        4x as fast. Expect: exit 0, both answers SAME, reads MEETS, writes MEETS, each pair's client
        and platform named."""
        memory = self.pair("memory", SAME, SAME)
        disk = self.pair("disk", {"create": 10, "insert": 400, "update": 800, "q1": 100, "q2": 200}, SAME)
        rc, out = verdict(self.pool, memory, disk)
        self.assertEqual(rc, 0, out)
        self.assertIn(f"memory: client {CLIENT}, platform {PLATFORM}, both runs", out)
        self.assertIn(f"disk: client {DISK_CLIENT}, platform {PLATFORM}, both runs", out)
        self.assertIn("answers: SAME: 0 of 5", out)
        self.assertIn("answers on disk: SAME: 0 of 5", out)
        self.assertIn("reads: MEETS", out)
        self.assertIn("writes: MEETS", out)

    def test_the_times_are_the_disks_not_memorys(self):
        """T-T1.4.1. Do: in memory versioned's writes are 4x as fast, on the disk no faster.
        Expect: writes MISSES: the verdict reads the disk's times."""
        memory = self.pair("memory", {"create": 10, "insert": 400, "update": 800, "q1": 100, "q2": 200}, SAME)
        disk = self.pair("disk", {"create": 10, "insert": 100, "update": 210, "q1": 100, "q2": 200},
                         {"create": 10, "insert": 105, "update": 200, "q1": 100, "q2": 200})
        rc, out = verdict(self.pool, memory, disk)
        self.assertEqual(rc, 0, out)
        self.assertIn("writes: MISSES: on the disk", out)

    def test_a_record_answered_otherwise_differs(self):
        """Do: versioned disagrees with SQLite on one query in memory, its answer on stdout.
        Expect: exit 1 and a `differ memory` line naming the record and both outcomes, with what it said."""
        shown = "a.test:10: SELECT a FROM t\n    SQLite answers 2\n    amphora answers 1\n"
        memory = self.pair("memory", SAME, SAME, v_outcome={"q1": "disagree"}, v_shown=shown)
        disk = self.pair("disk", SAME, SAME)
        rc, out = verdict(self.pool, memory, disk)
        self.assertEqual(rc, 1, out)
        self.assertIn("differ memory a.test:10: pager agree, versioned disagree", out)
        self.assertIn("amphora answers 1", out)
        self.assertIn("answers: DIFFER: 1 of 5", out)
        self.assertIn("answers on disk: SAME", out)

    def test_a_record_answered_otherwise_on_disk_differs(self):
        """Do: versioned disagrees on one query on the disk only. Expect: exit 1, `differ disk`."""
        memory = self.pair("memory", SAME, SAME)
        disk = self.pair("disk", SAME, SAME, v_outcome={"q2": "disagree"})
        rc, out = verdict(self.pool, memory, disk)
        self.assertEqual(rc, 1, out)
        self.assertIn("differ disk a.test:15: pager agree, versioned disagree", out)
        self.assertIn("answers: SAME", out)
        self.assertIn("answers on disk: DIFFER: 1 of 5", out)

    def test_a_record_in_one_run_only_is_partial(self):
        """T-T1.4.1. Do: versioned's disk run has no line for the last query (its file stopped short).
        Expect: exit 1, answers on disk PARTIAL, one record in one run only: not the whole corpus."""
        memory = self.pair("memory", SAME, SAME)
        disk = self.pair("disk", SAME, SAME, v_missing=("q2",))
        rc, out = verdict(self.pool, memory, disk)
        self.assertEqual(rc, 1, out)
        self.assertIn("answers: SAME: 0 of 5", out)
        self.assertIn("answers on disk: PARTIAL: 0 of 4 records reached by both differ (1 records in one run only, 0 unreached, 0 of the pool's 5 in neither run)", out)

    def test_an_unreached_record_is_partial(self):
        """T-T1.4.1. Do: both in-memory runs leave one record unreached (a budget). Expect: exit 1,
        answers PARTIAL, the record counted unreached, none differing."""
        memory = self.pair("memory", SAME, SAME, outcome_all={"q2": "unreached"})
        disk = self.pair("disk", SAME, SAME)
        rc, out = verdict(self.pool, memory, disk)
        self.assertEqual(rc, 1, out)
        self.assertIn("answers: PARTIAL: 0 of 4 records reached by both differ (0 records in one run only, 1 unreached, 0 of the pool's 5 in neither run)", out)
        self.assertIn("answers on disk: SAME", out)

    def test_both_stopped_at_the_same_record_is_partial(self):
        """T-T1.4.1. Do: both disk runs stop before the last query (no line for it in either), so
        every line they wrote agrees. Expect: exit 1, the pool counted at 5 records up front, answers
        on disk PARTIAL with the record in neither run: SAME needs every one of the pool's records."""
        memory = self.pair("memory", SAME, SAME)
        disk = self.pair("disk", SAME, SAME, missing_all=("q2",))
        rc, out = verdict(self.pool, memory, disk)
        self.assertEqual(rc, 1, out)
        self.assertIn("pool: 5 records in 1 files", out)
        self.assertIn("answers: SAME: 0 of 5", out)
        self.assertIn("answers on disk: PARTIAL: 0 of 4 records reached by both differ (0 records in one run only, 0 unreached, 1 of the pool's 5 in neither run)", out)

    def test_a_file_neither_run_reached_is_partial(self):
        """T-T1.4.1. Do: the pool holds a second file that no run wrote a line for (every run
        stopped before it). Expect: exit 1, the pool counted at 7 records in 2 files, both answers
        PARTIAL with its 2 records in neither run."""
        with open(os.path.join(self.pool, "b.test"), "w", encoding="utf-8") as f:
            f.write("statement ok\nCREATE TABLE u(b)\n\nquery I nosort\nSELECT 1\n----\n1\n")
        memory = self.pair("memory", SAME, SAME)
        disk = self.pair("disk", SAME, SAME)
        rc, out = verdict(self.pool, memory, disk)
        self.assertEqual(rc, 1, out)
        self.assertIn("pool: 7 records in 2 files", out)
        self.assertIn("answers: PARTIAL: 0 of 5 records reached by both differ (0 records in one run only, 0 unreached, 2 of the pool's 7 in neither run)", out)
        self.assertIn("answers on disk: PARTIAL", out)

    def test_the_pool_is_counted_as_the_run_reads_it(self):
        """T-T1.4.1. Do: the pool's file also holds a record SQLite skips and, after a halt SQLite
        obeys, one more; a halt under `onlyif mysql` before them is passed over; a base.test fixture
        and a file order.tsv leaves out sit beside it. Expect: still 5 records in 1 file, answers SAME:
        the count holds what `amphora-corpus run` reaches, no more."""
        with open(os.path.join(self.pool, "a.test"), "a", encoding="utf-8") as f:
            f.write("\nonlyif mysql\nhalt\n\nskipif sqlite\nstatement ok\nSELECT 1\n\nhalt\n\nstatement ok\nSELECT 2\n")
        os.makedirs(os.path.join(self.pool, "x"))
        for name in ("x/base.test", "x/left-out.test"):
            with open(os.path.join(self.pool, name), "w", encoding="utf-8") as f:
                f.write("statement ok\nSELECT 3\n")
        with open(os.path.join(self.pool, "order.tsv"), "w", encoding="utf-8") as f:
            f.write("required\ta.test\n")
        memory = self.pair("memory", SAME, SAME)
        disk = self.pair("disk", SAME, SAME)
        rc, out = verdict(self.pool, memory, disk)
        self.assertEqual(rc, 0, out)
        self.assertIn("pool: 5 records in 1 files", out)
        self.assertIn("answers: SAME: 0 of 5 records reached by both differ (0 records in one run only, 0 unreached, 0 of the pool's 5 in neither run)", out)

    def test_without_order_every_test_file_but_base_is_the_pools(self):
        """T-T1.4.1. Do: no order.tsv; the pool holds a fixture x/base.test and x/c.test besides
        a.test. Expect: 6 records in 2 files (the fixture isn't one), x/c.test's in neither run."""
        os.makedirs(os.path.join(self.pool, "x"))
        for name in ("x/base.test", "x/c.test"):
            with open(os.path.join(self.pool, name), "w", encoding="utf-8") as f:
                f.write("statement ok\nSELECT 3\n")
        memory = self.pair("memory", SAME, SAME)
        disk = self.pair("disk", SAME, SAME)
        rc, out = verdict(self.pool, memory, disk)
        self.assertEqual(rc, 1, out)
        self.assertIn("pool: 6 records in 2 files", out)
        self.assertIn("1 of the pool's 6 in neither run", out)

    def test_an_empty_pool_is_refused(self):
        """T-T1.4.1. Do: the pool holds no file, the runs no record. Expect: exit 2, refused, not SAME."""
        os.remove(os.path.join(self.pool, "a.test"))
        empty = {k: v for k, v in SAME.items()}
        memory = (run_dir(self.root, "memory-pager", empty, missing=tuple(LINES)), run_dir(self.root, "memory-versioned", empty, missing=tuple(LINES)))
        disk = (run_dir(self.root, "disk-pager", empty, client=DISK_CLIENT, missing=tuple(LINES)),
                run_dir(self.root, "disk-versioned", empty, client=DISK_CLIENT, missing=tuple(LINES)))
        rc, out = verdict(self.pool, memory, disk)
        self.assertEqual(rc, 2, out)
        self.assertIn("holds no record SQLite runs", out)
        self.assertNotIn("answers:", out)

    def test_the_count_alone_comes_before_the_runs(self):
        """T-T1.4.1. Do: `storages.py count` on the pool, then on an empty one (what storages.sh
        asks before its four runs). Expect: exit 0 with the pool's line, then exit 2, refused."""
        out = io.StringIO()
        argv = sys.argv
        try:
            with contextlib.redirect_stdout(out):
                sys.argv = ["storages.py", "count", self.pool]
                self.assertEqual(storages.main(), 0)
                empty = os.path.join(self.root, "empty")
                os.makedirs(empty)
                sys.argv = ["storages.py", "count", empty]
                self.assertEqual(storages.main(), 2)
        finally:
            sys.argv = argv
        self.assertIn("pool: 5 records in 1 files", out.getvalue())
        self.assertIn("pool: 0 records in 0 files\nrefused:", out.getvalue())

    def test_runs_of_another_pool_are_refused(self):
        """T-T1.4.1. Do: the pool given holds only a.test's first three records; the runs hold all
        five. Expect: exit 2, refused as another pool's runs, nothing compared."""
        with open(os.path.join(self.pool, "a.test"), "w", encoding="utf-8") as f:
            f.write(POOL.split("\nquery")[0])
        memory = self.pair("memory", SAME, SAME)
        disk = self.pair("disk", SAME, SAME)
        rc, out = verdict(self.pool, memory, disk)
        self.assertEqual(rc, 2, out)
        self.assertIn("refused: the memory runs hold 2 records the pool doesn't (a.test:10 first)", out)
        self.assertNotIn("answers:", out)

    def test_writes_no_faster_miss(self):
        """Do: on the disk versioned's writes take what the pager's take. Expect: writes MISSES (the bar is 3x)."""
        memory = self.pair("memory", SAME, SAME)
        disk = self.pair("disk", {"create": 10, "insert": 100, "update": 210, "q1": 100, "q2": 200},
                         {"create": 10, "insert": 105, "update": 200, "q1": 100, "q2": 200})
        rc, out = verdict(self.pool, memory, disk)
        self.assertEqual(rc, 0, out)
        self.assertIn("writes: MISSES", out)

    def test_another_platform_is_refused(self):
        """Do: a pair's versioned run names another platform. Expect: exit 2, refused, nothing compared."""
        memory = self.pair("memory", SAME, SAME)
        disk = self.pair("disk", SAME, SAME, v_platform="macos/aarch64")
        rc, out = verdict(self.pool, memory, disk)
        self.assertEqual(rc, 2, out)
        self.assertIn("refused: the disk runs'", out)
        self.assertNotIn("answers:", out)

    def test_pairs_from_two_machines_are_refused(self):
        """Do: the disk pair names one platform, the memory pair another. Expect: exit 2, not one machine."""
        mem = (run_dir(self.root, "memory-pager", SAME, platform="macos/aarch64"),
               run_dir(self.root, "memory-versioned", SAME, platform="macos/aarch64"))
        disk = self.pair("disk", SAME, SAME)
        rc, out = verdict(self.pool, mem, disk)
        self.assertEqual(rc, 2, out)
        self.assertIn("not one machine", out)
        self.assertNotIn("answers:", out)


if __name__ == "__main__":
    unittest.main()
