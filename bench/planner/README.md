# The planner bench: JOB and TPC-H, Amphora's estimates against the rows that ran

`amphora-bench --planner` (`bench/src/planner/`) loads a benchmark into Amphora, SQLite, DuckDB and
PostgreSQL, takes each one's statistics, and runs every query on each. On Amphora each query runs
once more on a connection that counts rows (`Connection::set_row_counters`, docs/design/planner.md
decision 12), and the record keeps, for every table access and join of every query and subquery,
the planner's estimate and the rows counted, beside the query's EXPLAIN QUERY PLAN FORMAT=JSON.
`summary.py` turns the records into the q-error at every join prefix, the q-error of each query's
largest intermediate result, and every engine's times.

## The benchmarks

- **JOB** (Leis et al., PVLDB 9(3), 2015): the IMDb tables of 2013 (`https://event.cwi.nl/da/job/imdb.tgz`,
  CSV as PostgreSQL's COPY writes them) and the 113 queries of gregrahn/join-order-benchmark, its
  `schema.sql` (primary keys) and `fkindexes.sql` (an index on each foreign key). `--queries` is a
  clone of that repository; the record names its commit.
- **TPC-H** (TPC-H 3.0.1): dbgen's tables at a scale factor (`tpch/dbgen.sh`, electrum/tpch-dbgen at
  a fixed commit) and its 22 queries with the specification's validation parameters (`tpch/queries/`,
  each date arithmetic done; Q15's view written as a WITH). The schema (`tpch/schema.sql`) gives each
  table its primary key; `tpch/indexes.sql` an index on each foreign key, the same on every engine.
  Amphora and SQLite keep TPC-H's decimals as REAL and its dates as ISO text, DuckDB and PostgreSQL
  as DECIMAL(15,2) and DATE; `YEAR(x)` in a query is each one's year of a date.

## The engines

Each engine runs a query on one thread, class for class with the embedded engines (CLAUDE.md §7):
DuckDB with `threads = 1` (`duckdb`; `duckdb-mt` is DuckDB with every core on a copy of its own, recorded beside it),
PostgreSQL with `max_parallel_workers_per_gather = 0`. Amphora and SQLite keep `--cache-mib` of
pages (2 GiB unless given); DuckDB and PostgreSQL keep what they ship with (`about` in the record).
PostgreSQL 17 runs in Docker (`postgres.sh`), its data on a Docker volume. Every query has a
deadline (`--timeout-s`, `--count-timeout-s` for Amphora's counted run): one that passes it is
stopped and recorded as timed out, and one an engine refuses is recorded with its words. DuckDB's
parser keeps `at` for itself, the alias JOB's 15a to 15d give aka_title, and refuses them as
written; it is given them with the alias quoted (`"at"`), the same query.

## Running it

```
bench/planner/tpch/dbgen.sh 1 ~/.cache/planner-bench/tpch-sf1
bench/planner/postgres.sh start
amphora-bench --planner --bench tpch --sf 1 --data ~/.cache/planner-bench/tpch-sf1 \
  --queries bench/planner/tpch/queries --dir ~/.cache/planner-bench/db-tpch-sf1 \
  --targets amphora,sqlite,duckdb,duckdb-mt,postgres --runs 3 --out tpch-sf1.jsonl
amphora-bench --planner --bench job --data ~/.cache/planner-bench/imdb \
  --queries ~/.cache/research-src/join-order-benchmark --dir ~/.cache/planner-bench/db-job \
  --targets amphora,sqlite,duckdb,duckdb-mt,postgres --runs 3 --out job.jsonl
python3 bench/planner/summary.py job.jsonl tpch-sf1.jsonl --json summary.json
```

A database is loaded the first time and kept: a load that finished leaves a `bench_loaded` table,
and one without it is dropped and loaded again. `record.sh` loads a benchmark in a process of its
own (`--only -`, which names no query) and times it in another: a pass timed by the process that had
just loaded its data ran slower (bench/records/2026-10-26-jupiter-planner-baseline.md). Each timed
run starts once the load is under `--max-load` (2 unless given), waiting at most `--load-wait-s`;
the load each began at is in its record. On Linux that is the one-minute load average; Windows keeps
none, so there it is the cores busy plus the processor queue, averaged over three seconds, which the
record names.

On a machine others use, a monitor beside the passes (every process's CPU each 10 s) and the passes'
log lines with their times let `disturbed.py` name the records that ran beside other work; those are
run again by a process of their own with the same binary (`--only`, `--targets`) once the machine is
quiet, into a record of their own, and a query's last record is its rerun
(bench/records/2026-10-26-jupiter-planner-baseline.md). `summary.py --best` takes each query's
fastest of the passes given. `test_disturbed.py` checks `disturbed.py` on logs as the passes write
them.

With `--answers <dir>`, each query's first timed run on each engine also writes its whole answer,
every row in the order the engine returned it, to `<dir>/<benchmark>/<query>/<engine>.tsv`
(`tpch-sf1/q16/duckdb.tsv`) beside the query's text (`query.sql`); that run's record says
`"captured": true`, as its time holds the writing, and the runs after it are timed as without. An
answer past 2^20 rows or 32 MiB, or a run that didn't finish, leaves a refusal saying why in its
place (`answers.rs`). `python3 -I answers.py <dir>` compares each query's answers between the
engines, and `answers.py <dir> <parent's dir>` each engine's against the parent build's too: row by
row where the query's outermost statement has ORDER BY (rows tied on its keys in any order), as
multisets elsewhere, REALs within a relative 1e-9 and text without CHAR(n)'s trailing spaces (its
docstring says why). It prints each difference with the first rows that differ, then `answers.py: N
queries compared, M differ`, and exits 1 if any differ. `ANSWERS=<dir>` has `record.sh` write each
pass's answers to `<dir>/<pass>`. `test_answers.py` checks `answers.py`.

A pass's queries are timed by one process, and DuckDB on all its threads runs slower the older that
process is: on jupiter, JOB's 16a, 16c, 17a and 32b took 2.4 to 4.9 times as long late in a pass
as in a process started for them. `repeat.py` compares passes timed alike, each by one process from start to
end, and such a pass stopped part way starts again from its beginning rather than resuming
(bench/records/2026-10-26-jupiter-planner-baseline.md).

## What is measured

- **A join prefix's rows**: the rows that left a table access into the next level of the join, or
  into the query's body after the last, having passed every term the plan evaluates there; for a
  correlated subquery, its rows over its calls, as its estimate is a call's (PostgreSQL's EXPLAIN
  ANALYZE does the same). A hash join's build table first in the order has its rows in the hash table
  as its prefix's. Any prefix without a count (a later hash build, a query that never ran, a counter
  with no estimate) is listed with why.
- **q-error**: max(est/act, act/est), each taken as at least one row (Moerkotte et al., PVLDB 2(1),
  2009), by the number of joins in the prefix (its position in the join order): median, 90th and 95th
  percentile (nearest rank), and maximum. Beside it, each query's largest intermediate result's
  q-error, since the q-error over every sub-plan misleads (Han et al., PVLDB 15(4), 2021).
- **Planning time**: Amphora's prepare (parse, plan and code), best of the runs. **Run time**:
  prepare and run to the last row, best of the runs; PostgreSQL's through its simple protocol, which
  plans and runs in one round trip.
- **Agreement**: each engine's row count for each query; any query on which they differ is listed.
  With `--answers`, every row of every answer (`answers.py`).
