# Baselines before optimizing

Goal 63c98840 ("Baseline numbers before optimizing") asks for Amphora's numbers in one session on a
quiet machine, beside DuckDB, ClickHouse and the leading row stores, and the commands that repeat
them. `run.sh <part>` gives each number from one command. Its records go to `bench/records` (or
`$OUT`) as `<date>-<host>-baseline-<part>*`, and each record keeps the machine, the commit and the
host's load. It then prints the part's summary (`check.py`). Each part runs a harness that already
existed, with the inputs and output it needs:

| Part | Number | Harness |
|---|---|---|
| `job-plan` | each JOB query's planning time on Amphora (prepare: parse, plan and code; the least of `RUNS`), its median, p99 and largest; **exits 1 if any query plans in over 20 ms**, the goal's bound | `amphora-bench --planner --bench job` |
| `job-search` | each JOB query's join search time within its planning, and the search's share of planning | the same, `search_ms` |
| `join-kinds` | index nested loops against hash joins, and each table's scan alone, at each build size | `bench/join_methods/run.sh` |
| `point-reads` | single-row reads by class (by primary key, one column, secondary index, text index, JSON field, blob, text) at one client: p50 and p99 on each row store, and Amphora's p50 over the leader's (the fastest other row store) | the suite (`bench/suite.sh`) |
| `scan-agg` | whole-table scans and aggregates in ns a row: the suite's (a scan no index serves, LIKE over long text, COUNT(*), GROUP BY over the table) on the row stores and ClickHouse, and TPC-H's q01 and q06 over lineitem's rows on Amphora, SQLite and DuckDB | the suite, and `amphora-bench --planner --bench tpch` |

`job-search` reads `amphora_engine::join_search_ns()`: the nanoseconds this thread has spent in the
join search (`compute_best_join_order_with_context`), a search inside another counted once.
The planner bench takes its difference across each prepare, and the record keeps it as `search_ms`
beside `prepare_ms`.

## The session (the coordinator's)

On a quiet machine, with `amphora-bench` built in release (`cargo build --release --manifest-path
bench/Cargo.toml --bin amphora-bench`), the IMDb CSVs in `$DATA/imdb`, dbgen's SF 1 tables in
`$DATA/tpch-sf1` (`bench/planner/tpch/dbgen.sh 1 $DATA/tpch-sf1`), and the suite's servers
(`bench/servers/`):

```
export CARGO_TARGET_DIR=<target dir> DATA=~/.cache/planner-bench JOB=~/.cache/research-src/join-order-benchmark
bench/baselines/run.sh job-plan
bench/baselines/run.sh job-search
bench/baselines/run.sh join-kinds
bench/baselines/run.sh point-reads --items 10000000 --seconds 5 --warmup 2
bench/baselines/run.sh scan-agg --items 10000000 --seconds 5 --warmup 2
```

Arguments after the part go to its harness: the planner bench's (`--only 1a,2a`, `--max-load`) for
the JOB parts, join-methods' rounds and sizes for `join-kinds`, the suite's for `point-reads` and
`scan-agg`. `TARGETS` names the engines (for `scan-agg`, the suite's; `TPCH_TARGETS` TPC-H's), `SF`
the TPC-H scale factor, `RUNS` the timed runs a query. `MAX_LOAD` lets `join-kinds` start above
the load of 2, for a tiny check on a busy host; such a run is no record.

`python3 -I check.py <plan|search|leader|scan> <records.jsonl>...` summarizes records already
written; `test_check.py` checks it on records as the harnesses write them.
