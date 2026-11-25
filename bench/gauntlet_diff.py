#!/usr/bin/env python3
"""Cross-engine differential verifier & performance report for `--gauntlet`.

Compares every named read op (~250) and every parametric query (1,024+) across
reference SQLite (or Postgres) and candidate engines (`amphora`, `amphora-pg`,
`postgres`), reporting:
  1. Exact answer digest mismatches (zero tolerance for silent wrong answers!)
  2. Named op failures / unsupported counts and worst-vs-reference ratios
  3. All 5 anti-gaming oracle results
  4. All 6 billions-of-agents live chaos workload results (service + e2e p99, audits)
"""

import json
import sys
from pathlib import Path


def load(path: str) -> dict:
    text = Path(path).read_text().strip()
    for line in reversed(text.splitlines()):
        line = line.strip()
        if line.startswith("{"):
            return json.loads(line)
    raise ValueError(f"no JSON object found in {path}")


def main() -> int:
    if len(sys.argv) < 3:
        print("usage: gauntlet_diff.py <reference.json> <candidate.json> [...]", file=sys.stderr)
        return 2

    ref = load(sys.argv[1])
    ref_name = ref.get("target", sys.argv[1])
    cands = [load(p) for p in sys.argv[2:]]
    exit_code = 0

    for cand in cands:
        cname = cand.get("target", "candidate")
        print(f"\n==================== {cname} vs {ref_name} (scale={cand.get('scale_label')}, rows={cand.get('rows_loaded')}, shards={cand.get('shards')}) ====================")
        print(f"Load: {cand.get('load_s')}s | Index: {cand.get('index_s')}s | Settle: {cand.get('settle_s')}s  (ref load={ref.get('load_s')}s idx={ref.get('index_s')}s)")

        # 1. Named Read Answer Parity
        r_ans = ref.get("named_answers", {})
        c_ans = cand.get("named_answers", {})
        c_ops = cand.get("named", {}).get("ops", {})
        r_ops = ref.get("named", {}).get("ops", {})
        checked_named = 0
        named_mismatches = []
        named_failures = []
        pg_targets = {"amphora-pg", "postgres", "orioledb", "cedardb"}
        cross_catalog = (cname in pg_targets) != (ref_name in pg_targets)
        for op, rv in r_ans.items():
            # Catalog queries differ legitimately between sqlite_schema & pg_catalog
            if op.startswith("cat_") and cross_catalog:
                continue
            if c_ops.get(op, {}).get("status") == "unsupported":
                continue
            if "error" in rv:
                named_failures.append((op, f"ref error: {rv['error']}"))
                continue
            checked_named += 1
            if op not in c_ans:
                named_failures.append((op, "candidate produced no answer digest"))
                continue
            cv = c_ans[op]
            if "error" in cv:
                named_failures.append((op, f"candidate error: {cv['error']}"))
                continue
            if rv.get("hash") != cv.get("hash") or rv.get("rows") != cv.get("rows"):
                named_mismatches.append((op, rv.get("rows"), cv.get("rows"), rv.get("sample"), cv.get("sample")))
        for op, cv in c_ans.items():
            if op not in r_ans and "error" in cv:
                named_failures.append((op, f"candidate error: {cv['error']}"))

        named_exact = checked_named - len(named_mismatches) - sum(1 for op, _ in named_failures if op in r_ans and "error" not in r_ans[op])
        print(f"\n[1] Named Read Parity: {named_exact}/{checked_named} exact matches (failures={len(named_failures)})")
        for op, msg in named_failures[:20]:
            print(f"  FAILED   {op}: {msg}")
        for op, rr, cr, rs, cs in named_mismatches[:20]:
            print(f"  MISMATCH {op}: ref_rows={rr} cand_rows={cr}")
            print(f"    ref sample:  {rs[:2] if rs else rs}")
            print(f"    cand sample: {cs[:2] if cs else cs}")
        if named_mismatches or named_failures:
            exit_code = 1
        if r_ans and checked_named == 0 and any(not (op.startswith("cat_") and cross_catalog) and c_ops.get(op, {}).get("status") != "unsupported" for op in r_ans):
            print("  FAILED   reference answered named queries but candidate compared 0")
            exit_code = 1

        # 2. Parametric 1,024+ Query Answer Parity
        r_par = ref.get("parametric_digests", {})
        c_par = cand.get("parametric_digests", {})
        c_par_meta = cand.get("parametric", {})
        checked_par = 0
        par_mismatches = []
        par_failures = []
        for qid, rv in r_par.items():
            if "error" in rv:
                par_failures.append((qid, f"ref error: {rv['error']}"))
                continue
            checked_par += 1
            if qid not in c_par:
                par_failures.append((qid, "candidate produced no digest (query failed or omitted)"))
                continue
            cv = c_par[qid]
            if "error" in cv:
                par_failures.append((qid, f"candidate error: {cv['error']}"))
                continue
            if rv.get("hash") != cv.get("hash") or rv.get("rows") != cv.get("rows"):
                par_mismatches.append((qid, rv.get("rows"), cv.get("rows")))
        for qid, cv in c_par.items():
            if qid not in r_par and "error" in cv:
                par_failures.append((qid, f"candidate error: {cv['error']}"))
        reported_par_failed = c_par_meta.get("failed", 0)
        par_exact = checked_par - len(par_mismatches) - sum(1 for qid, _ in par_failures if qid in r_par and "error" not in r_par[qid])
        print(f"[2] Parametric Query Parity: {par_exact}/{checked_par} exact matches (failed={reported_par_failed}, diff_failures={len(par_failures)})")
        if reported_par_failed:
            print(f"  FAILED   parametric runner reported {reported_par_failed} failed queries (first_error={c_par_meta.get('first_error')})")
        for qid, msg in par_failures[:20]:
            print(f"  FAILED   {qid}: {msg}")
        for qid, rr, cr in par_mismatches[:20]:
            print(f"  MISMATCH {qid}: ref_rows={rr} cand_rows={cr}")
        if par_mismatches or par_failures or reported_par_failed > 0:
            exit_code = 1
        if r_par and checked_par == 0:
            print("  FAILED   reference answered parametric queries but candidate compared 0")
            exit_code = 1

        # 3. Named Op Errors & Speed Ratios vs Reference
        ratios = []
        errs = []
        unsup = []
        for op, cv in c_ops.items():
            if cv.get("status") == "unsupported":
                unsup.append(op)
                continue
            if cv.get("errors", 0) > 0:
                errs.append((op, cv.get("first_error")))
                continue
            rv = r_ops.get(op)
            if rv and rv.get("ops_per_s", 0) > 0 and cv.get("ops_per_s", 0) > 0:
                ratios.append((cv["ops_per_s"] / rv["ops_per_s"], op, cv["domain"], cv["ops_per_s"], rv["ops_per_s"]))
        ratios.sort(key=lambda x: x[0])
        reported_named_failed = cand.get("named", {}).get("failed", 0)
        print(f"[3] Named Ops: passed={cand.get('named', {}).get('passed')} failed={max(len(errs), reported_named_failed)} unsupported={len(unsup)}")
        for op, msg in errs[:25]:
            print(f"  ERROR {op}: {msg}")
        if unsup:
            print(f"  Unsupported ({len(unsup)}): {', '.join(unsup[:15])}{'...' if len(unsup) > 15 else ''}")
        if ratios:
            print(f"  Slowest 15 ops vs {ref_name}:")
            for rat, op, dom, co, ro in ratios[:15]:
                print(f"    {rat:6.2f}x  {op:34s} [{dom:13s}] cand={co:9.1f}  ref={ro:9.1f} ops/s")
            print(f"  Fastest 5 ops vs {ref_name}:")
            for rat, op, dom, co, ro in ratios[-5:]:
                print(f"    {rat:6.2f}x  {op:34s} [{dom:13s}] cand={co:9.1f}  ref={ro:9.1f} ops/s")
        if errs or reported_named_failed > 0:
            exit_code = 1

        # 4. Oracles
        print("[4] Anti-Gaming Oracles:")
        for oname, ov in cand.get("oracles", {}).items():
            ok = ov.get("passed", False)
            print(f"  {'PASS' if ok else 'FAIL'}  {oname}")
            if not ok:
                print(f"        details: {ov.get('details')}")
                exit_code = 1

        # 5. Billions-of-Agents Chaos
        chaos = cand.get("chaos", [])
        if chaos:
            print("[5] Billions-of-Agents Concurrent Chaos:")
            for sc in chaos:
                r_p99 = (sc.get("short_read_service_ms") or {}).get("p99")
                re_p99 = (sc.get("short_read_e2e_ms") or {}).get("p99")
                w_p99 = (sc.get("write_service_ms") or {}).get("p99")
                ok = sc.get("errors", 0) == 0 and sc.get("audit_failures", 0) == 0
                print(
                    f"  {'PASS' if ok else 'FAIL'}  {sc['scenario']:26s} "
                    f"ops/s={sc['ops_per_s']:8.1f} read_p99={r_p99}ms (e2e={re_p99}ms) "
                    f"write_p99={w_p99}ms audits={sc['audit_checks']} fails={sc['audit_failures']} errs={sc['errors']}"
                )
                if not ok:
                    print(f"        err={sc.get('first_error')} audit_fail={sc.get('first_audit_failure')}")
                    exit_code = 1

    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
