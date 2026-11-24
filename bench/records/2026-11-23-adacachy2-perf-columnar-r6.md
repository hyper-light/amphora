# Lane R6: the column copy choice weighs both paths as measured (2026-11-23, adacachy2)

Ada's goal is Amphora about ten times faster than DuckDB and SQLite on analytics. The column
copies exist to make whole-table statements fast, but the planner sent almost nothing to them.

Verdict, losses first.
- `ab::cb4_ab_both_rebuy_policies_at_the_extremes` still fails at its waves check (`dropped` 0),
  before the fix and after. The cause is not routing. After the fix every statement a copy could
  answer after rent and purchase went to the copy (128 copy statements, 64 before), and the memory
  source refused none of the copies' 320 takes. At scale 1 the waves scenario has four rounds over
  64 tables. Round 2 is the only pressed round in which a copy is read, and it leaves the copies
  half a copy of room, which their catch-up takes fit in. Read from the scenario's schedule
  (`columns_ab.rs`, `free_at`) and the record's counts. Not fixed (rule 0).
- The A/B on S8's analytics statements was not run in this lane's hour. Its script is ready (below).
- `cb4_a_read_only_transaction_reads_the_copy_at_its_snapshot` passes on merit: the copy measured
  faster than the rows on the same basis, and nothing forces the path.

## The cause, traced before the fix

`AMPHORA_PLAN_TRACE=1` prints a debug event (`amphora::column_path`) at each SQL aggregate's choice,
with both costs (commit e728b863: the event and the tests' subscriber, no change to the choice).
kiln r1805, adacachy2, debug test build, 4 test threads, load 24-70.

cb4_a's statements on its 2,000-row table, in order:

| statement | copy ns/row | rows ns/row | copy cost ns | rows cost ns | path |
|---|---|---|---|---|---|
| 2nd (copy bought, no generation yet) | none | 7.57 | none | 0 | Rows |
| 3rd | none (unmeasured) | 7.57 | 15,136 | 15,136 | Copy |
| 4th | none (first scan not charged) | 7.57 | 15,136 | 15,136 | Copy |
| 5th to 7th | 856.99 | 7.57 | 1,713,971 | 15,136 | Rows |

The rows' 7.57 ns a row is calibration's `Work::Row`, a filter-and-count over in-memory `u64`s
that no query ever updates. The copy's 857 ns is its whole aggregate's wall time per row of the
table. Once the copy was measured, the rows won every statement. On main 0bb4b917 itself (r1794)
the figures were 510 ns against 5.16 ns, with the same failure.

## After the fix (cb44211b, kiln r1845)

The rows are timed on their own serial loop for the statements the copies hand back, per table,
per row of the table (the copy's view when exact, else the rows the loop read). The time counted
is the engine's while the statement steps, not while its caller holds a row.

| statement | copy ns/row | rows ns/row | copy cost ns | rows cost ns | path |
|---|---|---|---|---|---|
| 2nd | none | 8,857 | none | 0 | Rows |
| 3rd, 4th | none | 7,484 | 14,967,685 | 14,967,685 | Copy |
| 5th to 9th | 701 to 988 | 7,484 | 1,402,019 to 1,976,599 | 14,967,685 | Copy |

The rows' serial loop costs about 7.5 microseconds a row in this debug build, about 1,000 times
the calibration figure it was priced at. The copy wins by about 7.6 to 10.7 times on this table,
so the copy is chosen because it measured faster. cb4_a passes (1 passed). cb4_ab's records keep
`"equal": true` in every scenario, so no answer changed.

R5's `SELECT g, sum(x) FROM r GROUP BY g` over a REAL column made no choice event in r1805's
trace, though it runs three times with the copies on. It is handed back before any cost is
weighed (by the hook's shape or a skip), so it is not a routing choice. Not traced further.

## Runs

- r1794: main 0bb4b917, cb4_a and cb4_ab, both fail (copy 510.20 ns/row, rows 5.16 ns/row).
- r1805: e728b863 (trace only), `AMPHORA_PLAN_TRACE=1`, both fail; the trace above.
- r1845: cb44211b (the fix), `AMPHORA_PLAN_TRACE=1`: cb4_a ok, cb4_ab fails at the waves check.
- r1891, the merge gate on 15449390 (the fix merged into main 2f29b288):
  - `storage::versioned::columns_sql::`, `columns_path::` and `translate::optimizer::column_path::`:
    23 passed, 1 failed (cb4_ab, as on main).
  - `cargo clippy -p amphora-engine --all-targets --locked -- -D warnings` exit 0.
  - `amphora --test columnar_decision`: `cs_1_every_claim_in_the_columnar_decision_is_cited` fails.
- r1904: that doc test at 15449390 and at main 2f29b288 gives the same nine faults, all in section
  2's item 4 ("Which way is the default is open": 64, 64, 1, 8, 32, 128, 512, 2048, 4096 in no
  cited quote). The failure is main's own, and the new part 6 item adds none.
- Merged to main as 50ab86c6. Main had moved only in `crates/pg-parser`, which neither
  amphora-engine nor amphora depends on.

## The A/B, not run

It was refused at 00:35Z and 00:36Z: adacachy2 had 0 of 32 cores idle, at cpus=16 and again at 4.
r1921 started at cpus=4 at 00:38Z. Its build stopped at once: the bench workspace has no
`Cargo.lock` in git, so `--locked` refuses to make one. The script now builds without `--locked`,
and the second arm builds on the lock the first arm resolved. The relaunch at 00:42Z was refused
(2 cores idle). Two commands are ready in `~/.cache/amphora-perf/reals-r6-scripts/` on adacachy:
- `ab_build_cmd.txt` builds both arms, loads the data and prints each statement's path counts,
  with ROUNDS=0 at cpus=4.
- `ab_cmd.txt` runs the whole A/B: 5 alternated rounds at cpus=16.
The `vdbe::morsel` suites at cb44211b (r1845's second half) were still running at 00:43Z.

`~/.cache/amphora-perf/reals-r6-scripts/ab_linux.sh` (lane R6's scripts, adacachy) runs S8's five
analytics statements (`table_count`, `table_group_by`, `filter_no_index`, `aggregates_range`,
`join_three`) at 1 and 32 clients on both arms, alternated, with one calibration for both
(RULES 5e). It also prints each statement's path counts from a separate traced pass.
- Arm A 3c0e1c74: main 0bb4b917, plus the trace event and the suite's `AMPHORA_PLAN_TRACE`
  subscriber.
- Arm B 44f70ed2: arm A plus the fix.
- The two arms differ only by the fix.

The suite's Amphora target runs on the pager's B-trees with the copies off unless
`AMPHORA_STORAGE=versioned AMPHORA_COLUMN_PATH=on` are set. So S8's own run never reached a copy,
and the script sets both on both arms. Command:
`A=3c0e1c74 B=44f70ed2 ROUNDS=5 JOBS=16 SECONDS_=2 bash ab_linux.sh`, with kiln needs
`linux/amd64,cpus=16,mem=24g` on adacachy2.
