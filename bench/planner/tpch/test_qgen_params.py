#!/usr/bin/env python3
"""qgen_params.py, TPC-H's parameters bound to the prepared templates (PL.12, AC-PL.12.1's harness),
on the log qgen -l wrote for seed 7 at scale factor 1 (electrum/tpch-dbgen 32f1c1b).

usage: python3 -I bench/planner/tpch/test_qgen_params.py
"""

import importlib.util
import os
import re
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
_spec = importlib.util.spec_from_file_location("qgen_params", os.path.join(HERE, "qgen_params.py"))
qgen_params = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(qgen_params)

# qgen -s 1 -r 7 -l, its 22 lines as written.
LOG = "\n".join([
    "1\t116", "2\t15\tNICKEL\tEUROPE", "3\tBUILDING\t1995-03-23", "4\t1996-08-01",
    "5\tASIA\t1994-01-01", "6\t1994-01-01\t0.08\t25", "7\tCHINA\tJORDAN",
    "8\tCHINA\tASIA\tLARGE BURNISHED NICKEL", "9\tmedium", "10\t1994-06-01",
    "11\tMOROCCO\t0.0001000000", "12\tFOB\tAIR\t1994-01-01", "13\tspecial\trequests",
    "14\t1994-11-01", "15\t1996-06-01",
    "16\tBrand#41\tECONOMY ANODIZED\t24\t47\t41\t9\t28\t2\t37\t35", "17\tBrand#14\tMED BAG",
    "18\t314", "19\tBrand#35\tBrand#51\tBrand#35\t7\t17\t26", "20\ttan\t1997-01-01\tCANADA",
    "21\tSAUDI ARABIA", "22\t23\t33\t43\t38\t37\t36\t22",
]) + "\n"


class Bound(unittest.TestCase):
    def test_pl_12_1_each_query_binds_what_its_template_asks(self):
        """Do: bind seed 7's log. Expect: for each of the 22, as many values as its template's
        highest ?N, the template being read from bench/planner/tpch/prepared."""
        logged = qgen_params.parse(LOG)
        self.assertEqual([q for q, _ in logged], list(range(1, 23)))
        for q, p in logged:
            with open(os.path.join(HERE, "prepared", f"q{q:02}.sql")) as f:
                highest = max(int(n) for n in re.findall(r"\?(\d+)", f.read()))
            values = qgen_params.bound(q, p)
            self.assertEqual(len(values), highest, f"q{q:02}")
            self.assertEqual(qgen_params.ARITY[q], highest, f"q{q:02}")

    def test_pl_12_1_the_spec_arithmetic_on_a_parameter(self):
        """Do: bind Q1, Q4, Q6, Q13, Q16 and Q20. Expect: TPC-H §2.4's dates, ranges and patterns."""
        logged = dict(qgen_params.parse(LOG))
        self.assertEqual(qgen_params.bound(1, logged[1]), ["1998-08-07"])
        self.assertEqual(qgen_params.bound(4, logged[4]), ["1996-08-01", "1996-11-01"])
        self.assertEqual(qgen_params.bound(6, logged[6]), ["1994-01-01", "1995-01-01", 0.07, 0.09, 25])
        self.assertEqual(qgen_params.bound(10, ["1994-11-01"])[1], "1995-02-01")
        self.assertEqual(qgen_params.bound(13, logged[13]), ["%special%requests%"])
        self.assertEqual(qgen_params.bound(16, logged[16])[:3], ["Brand#41", "ECONOMY ANODIZED%", 24])
        self.assertEqual(qgen_params.bound(20, logged[20]), ["tan%", "1997-01-01", "1998-01-01", "CANADA"])


if __name__ == "__main__":
    unittest.main()
