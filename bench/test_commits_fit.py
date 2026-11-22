"""commits_fit.py's checks of a writer count against the host (goal "Many clients write through SQL
at once, safely"; T-T1.4.1). Run: python3 -m unittest bench/test_commits_fit.py"""
import contextlib
import io
import json
import os
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import commits_fit  # noqa: E402

KB, MB, GB = 1024, 1024**2, 1024**3


class Fit(unittest.TestCase):
    def test_need_from_the_rungs_already_run(self):
        """Do: predict at 0, 1 and 2 rungs run. Expect: nothing assumed before a rung; one rung's
        whole cost a writer's; with two, the line through the two largest."""
        self.assertIsNone(commits_fit.predict([], "driver_fds", 8))
        one = [(1, {"driver_fds": 6})]
        self.assertEqual(commits_fit.predict(one, "driver_fds", 8), 48)
        two = [(1, {"driver_fds": 6}), (8, {"driver_fds": 27}), (32, {"driver_fds": 99})]
        self.assertEqual(commits_fit.predict(two, "driver_fds", 128), 387)

    def test_pg_settings_fit_the_room(self):
        """Do: size PostgreSQL for 32 and for 4096 writers. Expect: with room, shared_buffers a
        quarter of memory and work_mem its 4MB default; tight, both shrink to fit; no room, None."""
        roomy = commits_fit.pg_settings(64 * GB, 120 * GB, 32, 200 * MB, 32 * 10 * MB)
        self.assertEqual(roomy, {"shared_buffers_kb": 16 * GB // KB, "work_mem_kb": 4096})
        tight = commits_fit.pg_settings(64 * GB, 20 * GB, 4096, 1 * GB, 4096 * 4 * MB)
        self.assertLess(tight["shared_buffers_kb"], 16 * GB // KB)
        need = tight["shared_buffers_kb"] * KB + 4096 * tight["work_mem_kb"] * KB + 1 * GB + 4096 * 4 * MB
        self.assertLessEqual(need, 20 * GB)
        self.assertIsNone(commits_fit.pg_settings(64 * GB, 2 * GB, 4096, 1 * GB, 4096 * MB))

    def check(self, rungs, system, writers, mem=(64 * GB, 8 * GB), threads=(100000, "test"), **kw):
        with tempfile.TemporaryDirectory() as d:
            learned = os.path.join(d, "learned.jsonl")
            with open(learned, "w", encoding="utf-8") as f:
                for w, v in rungs:
                    f.write(json.dumps({"system": system, "writers": w, **v}) + "\n")
            saved = commits_fit.meminfo, commits_fit.threads_room
            commits_fit.meminfo, commits_fit.threads_room = (lambda: mem), (lambda: threads)
            out = io.StringIO()
            try:
                with contextlib.redirect_stdout(out):
                    rc = commits_fit.check(learned, system, writers, **kw)
            finally:
                commits_fit.meminfo, commits_fit.threads_room = saved
            return rc, json.loads(out.getvalue())

    def test_a_count_past_the_memory_is_refused(self):
        """Do: 1 and 8 writers took 10 and 80 MiB; ask for 4096 in 8 GiB. Expect: refused, saying
        what it would take and what's available; 32 writers fit."""
        rungs = [(1, {"driver_rss_bytes": 10 * MB}), (8, {"driver_rss_bytes": 80 * MB})]
        rc, v = self.check(rungs, "versioned", 4096)
        self.assertEqual(rc, 2)
        self.assertIn("40960 MiB", v["why"])
        self.assertIn("8192 MiB is available", v["why"])
        rc, v = self.check(rungs, "versioned", 32)
        self.assertEqual((rc, v["fits"]), (0, True))

    def test_a_count_past_the_threads_is_refused(self):
        """Do: ask for more writers than threads a process may start. Expect: refused, the bound named."""
        rc, v = self.check([], "sqlite", 4096, threads=(2048, "kern.num_taskthreads"))
        self.assertEqual(rc, 2)
        self.assertIn("kern.num_taskthreads", v["why"])

    def test_postgres_sized_or_refused(self):
        """Do: PostgreSQL at 4096 writers whose backends took 8 MiB each, in 64 and in 16 GiB.
        Expect: in 64, settings that fit; in 16, refused, saying its backends leave no room."""
        rungs = [(1, {"server_bytes": 150 * MB, "driver_rss_bytes": 10 * MB}),
                 (8, {"server_bytes": 206 * MB, "driver_rss_bytes": 12 * MB})]
        rc, v = self.check(rungs, "postgres", 4096, mem=(128 * GB, 64 * GB), pg_shared=1 * GB)
        self.assertEqual(rc, 0, v)
        self.assertGreater(v["pg"]["work_mem_kb"], 64)
        rc, v = self.check(rungs, "postgres", 4096, mem=(32 * GB, 16 * GB), pg_shared=1 * GB)
        self.assertEqual(rc, 2, v)
        self.assertIn("backends", v["why"])


if __name__ == "__main__":
    unittest.main()
