#!/usr/bin/env python3
"""disturbed.py on logs written as the passes write them: run it on a pass whose lines follow a
pass start with no `==>` header (tail following one log writes none), expect its records counted as
its own, and the pass before it keeping only its own.

usage: python3 -I bench/planner/test_disturbed.py [<disturbed.py>]
"""

import os
import subprocess
import sys
import tempfile
import unittest

SCRIPT = sys.argv.pop(1) if len(sys.argv) > 1 else os.path.join(os.path.dirname(os.path.abspath(__file__)), "disturbed.py")

MONITOR = """\
2026-10-28T06:03:30Z total=0.60 freeGB=400.0 amphora-bench:1:0.50
2026-10-28T13:32:00Z total=0.60 freeGB=400.0 amphora-bench:1:0.50
2026-10-28T13:32:10Z total=0.60 freeGB=400.0 amphora-bench:1:0.50
2026-10-28T13:32:20Z total=6.60 freeGB=400.0 amphora-bench:1:0.50 rustc:2:6.00
"""

PASSES = """\
2026-10-28T06:03:37Z ==> /c/records/tpch-sf10-c-again.log <==
2026-10-28T06:03:37Z q13 duckdb-mt: 681.8 ms
2026-10-28T13:31:36Z == job-e
2026-10-28T13:32:06Z 1a amphora: 408.3 ms
2026-10-28T13:32:20Z 1a sqlite: 187.1 ms
"""


def run(pass_name, *extra):
    with tempfile.TemporaryDirectory() as d:
        mon, ts = os.path.join(d, "monitor.log"), os.path.join(d, "passes-ts.log")
        with open(mon, "w") as f:
            f.write(MONITOR)
        with open(ts, "w") as f:
            f.write(PASSES)
        out = subprocess.run([sys.executable, "-I", SCRIPT, mon, ts, pass_name, *extra], capture_output=True, text=True, check=True)
        return out.stdout.splitlines()[0]


class Disturbed(unittest.TestCase):
    def test_a_pass_whose_log_has_no_header_counts_its_own_records(self):
        self.assertEqual(run("job-e"), "job-e: 2 records, 1 with other work above 2.0 cores or in a window")

    def test_the_pass_before_keeps_only_its_own_records(self):
        self.assertEqual(run("tpch-sf10-c-again"), "tpch-sf10-c-again: 1 records, 0 with other work above 2.0 cores or in a window")

    def test_a_pass_started_again_counts_only_what_came_after_since(self):
        self.assertEqual(run("job-e", "--since", "2026-10-28T13:32:10Z"), "job-e: 1 records, 1 with other work above 2.0 cores or in a window")


if __name__ == "__main__":
    unittest.main()
