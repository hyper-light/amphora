# libpg_query's cost per statement, the parser's base case (lane PG1, 2026-11-23, MacBook Pro 2)

The base case for `docs/design/postgres.md` section 3: what PostgreSQL's parser costs when it is
extracted into a C library, as pganalyze's libpg_query extracts it, before any Rust binding decodes
its output.

**Verdict: inconclusive in absolute terms.** The machine ran other lanes' builds throughout (load
average 30 to 58 on 18 cores), so these are timeslices of an oversubscribed machine, not idle
numbers. One finding held in every run and is the one the design uses: encoding the tree to protobuf
costs 3 to 5 times as much as parsing it.

## The run

- Machine: MacBook Pro 2 (kiln runner `e30189ea29182644fe5a4b2e06c7dac2`), Apple M5 Max, 18 cores,
  128 GiB, macOS (Darwin 25.4), Apple clang 21.
- libpg_query branch `18-latest` at `7663e07d` (PostgreSQL 18.6's parser, upb protobuf), built
  `-O3` by its own Makefile; a second copy with `-DDEBUG`, which removes only the per-call stderr
  redirection, alternated with it.
- Corpus: PostgreSQL `REL_19_0`'s `src/test/regress/sql/*.sql` (245 files), split with
  `pg_query_split_with_scanner`: 53,504 statements, 4.92 MB. Four files failed to split and gave
  nothing (`copy2.sql`, `create_am.sql`, `numerology.sql`, `strings.sql`).
- 51,739 statements parse (4.68 MB, about 90 bytes each); 1,765 fail (deliberate syntax errors,
  PostgreSQL 19-only syntax, psql commands and COPY data, not told apart). Every timed pass gave the
  same counts and the same protobuf bytes (7.72 MB), so the work ran inside the timed region.
- Harness, scripts and commits: `~/.cache/amphora-perf/pg-parse-bench` (`pgbench_corpus.c`,
  `run.sh`, `run2.sh`; commits from `c60f3d9`), run by kiln `run` with needs
  `macos/arm64,cpus=4,mem=16g`, single-threaded, 5 runs, the order rotated every round.

## The numbers, nanoseconds a statement over the statements that parse

| What | Median of 5 run medians | Range over 47 rounds | Least-loaded run |
|---|---|---|---|
| `pg_query_parse_protobuf` and free | 11,177 | 4,081 to 32,654 | 6,503 |
| `pg_query_raw_parse` alone | 2,441 | 746 to 7,032 | 1,244 |
| raw parse, then encode to protobuf | 8,780 | 4,408 to 38,071 | 6,713 |
| `pg_query_parse` (JSON) and free | 6,352 | 2,310 to 21,829 | 3,308 |

Load average beside the five runs, oldest first: 45.8, 43.9, 50.6, 47.5, 40.0. The repository's own
`benchmark/bench_protobuf` (one run, 20,000 iterations, load 38): 25,199 ns for `parse_protobuf`
on its ten queries, 3.3 MB peak memory.

## What limits it

Encoding to protobuf took 5.5 to 10.6 µs a statement against 1.2 to 3.5 µs of parsing, in every run:
about three quarters of what a Rust binding pays before it decodes anything. The JSON path writes
four times the bytes at half the protobuf path's cost, so the limiter is the per-node work of
building protobuf messages, not the bytes or the grammar (inferred from these differences; no
profiler was run).

## Not checked

Idle-machine numbers; a profile; the Rust binding's protobuf decode (pg_query.rs); Linux. To be
rerun on an idle machine, alternating with the Rust parser of section 3 once it exists, on the same
statements.
