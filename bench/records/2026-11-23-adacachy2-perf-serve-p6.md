# Lane P6 (serve): a caller's session off the owner's thread, designed (2026-11-23, adacachy2)

Goal: a replicated server that answers writes of any size and length; every write commits or is refused typed,
and one long write never costs the group its leader (CP-T1.13). Lane P5 found the cause
(2026-11-23-adacachy2-perf-serve-p5.md). The coordinator chose P5's option (a): the session on a worker of its
own. This lane wrote the design (docs/design/replication.md, "Decision (lane P6)") and built none of it in its hour.

## What was run

Through kiln `run`, `on` adacachy2 (00aad8da...), needs `linux/amd64,cpus=8,mem=24g`, CARGO_INCREMENTAL=0, debug
test build, script ~/.cache/amphora-perf/serve-p6-scripts/run.sh:

    run.sh 0b120397 test --test serve cp_t1_13     (r917; main 9a7b14bb + P5's probes and failing test)

| Test | Result |
|---|---|
| `cp_t1_13_a_replicated_write_past_an_election_timeout_commits_and_keeps_its_leader` (P5's, 16 MiB) | FAILED at serve.rs:855, `wrote.is_ok()`, as P5 recorded |
| `cp_t1_13_sql_takes_bound_parameters_and_answers_typed_rows_with_their_names` (main's own) | FAILED at serve.rs:735, the `unwrap` of one write of 20 rows of 1 MiB |
| the other two `cp_t1_13` tests in tests/serve.rs | ok |

Load average 47.3, 61.2, 59.9 when the run began. The second failure is main's own test, untouched by this lane.
Its failing line is a 20 MiB replicated write, the shape P5 traced to the session outlasting the election timeout;
the error text wasn't kept (the next run's log replaced it), so that it is the same cause is inferred, not shown.
It is a second test the design's fix has to turn green.

    run.sh df257430 test --lib -- the_design_names the_decisions_numbers     (r924; the doc-truth tests that read
    replication.md, on the design's commit)

## Not measured

Commit latency at 1 and 32 writers on the replicated rung, before and after: nothing was built to compare. The
successor measures both arms from fixed SHAs with one shared XDG_CACHE_HOME (RULES 5e).
