# Lane PG16 (postgres), 2026-11-24: unit 8, analysis, built and not merged

Hour: 2026-11-24T05:26:17Z to 06:26:17Z (from the lane's first command). Branch `perf/postgres-pg16`
from main 1f4a8e88. Not a speed change: no A/B, nothing timed. Ada's benchmark window took adacachy2 at
05:57Z; the lead's word was to push the branch finished and unmerged.

## Losses first

- Not merged. check-production has not run on this branch: it counts only on Linux, and the window
  closed Linux to the lane.
- Not run on this branch: amphora-pg-catalog's own tests (its generated data changed: they hold
  data.rs's hash and pg_type's rows), amphora-engine's clippy and tests (it depends on
  amphora-pg-catalog, whose `Proc` gained fields). amphora-pg-catalog's clippy passed, tests included.
- No red run: the crate is new, so its tests don't compile on main.
- Functions: `resolve::function` is checked only by the calls in statements.txt. A sweep of 14,382
  calls against the server is written and its answers taken (3,340 resolve or are refused some way
  other than `42883`), but the test has never run: branch `perf/postgres-pg16-unrun`.
- The server's catalog reads `"$user", public` with no schema of the role's name. No test has a schema
  before pg_catalog, or an operator or function a user made: there is no CREATE OPERATOR, FUNCTION or
  SCHEMA to make one.
- The expected values come from Debian's 19.0 image. It refused `US/Eastern` with `22023`, which 19's own
  tzdata.zi has as a link, so I infer it reads the system's tzdata and not 19's file (not checked). That
  zone was dropped from the list; every zone and instant left agrees with 19's file.

## What the unit can't do yet

- Choosing an operator or function with polymorphic arguments refuses `0A000`:
  `enforce_generic_type_consistency` isn't ported. Choosing among them is.
- No body for LIKE and regular expressions, text search, or anything that gives an interval or a
  numeric. Resolving to one refuses `0A000` naming it.
- A timestamp literal in a form other than ISO's refuses `0A000`, and so do POSIX and INTERVAL
  `TimeZone` settings.
- CURRENT_TIMESTAMP is typed, not computed: analysis has no clock.
- What the parser's ported rules don't read couldn't be asked as a statement: `::text` (its keyword;
  the tests write `::"text"`), a FROM of several tables, an alias without AS, VALUES of several rows,
  INSERT without columns, UPDATE of several columns or without WHERE. Their analysis is left out.

## The checks against the server

PostgreSQL 19.0 (Debian 19.0-1.pgdg13+1), the postgres:19 image bb6e70add742 in a container of the
lane's own on adacachy2 (bench/servers/postgres.sh's container name is shared, so not that one),
removed at 05:56Z.

| What | Asked | Agree |
|---|---|---|
| Each operator name between each pair of the unit's types and unknown (T-PG.8.1) | 5,697 (638 resolve, 72 are `42725`, 4,987 are `42883`) | 5,697 |
| Each cast from them to 24 types: the explicit one's node, assignment, implicit (T-PG.8.1) | 216 | 216 |
| Statements: parameter and column types and typmods, SQLSTATEs, values (T-PG.8.2) | 441, of which 180 under a `TimeZone` other than UTC | 441 |

The last run of scripts/postgres-answers.sh (05:54Z) wrote the three files as they are checked in.
The brief's cases: `SELECT 1` is int4; `SELECT 2147483647 + 1` is int4 and `22003`;
`SELECT $1::pg_catalog.regclass` takes a regclass, and `pgbench_accounts` resolves; INSERT and UPDATE of a
timestamp into a timestamptz column and back under `-8`, `5.5`, America/New_York, Europe/London and
Asia/Kolkata.

## Runs

All on adacachy2 (`sync: false` from the GitHub clone; CARGO_HOME, CARGO_TARGET_DIR
~/.cache/amphora-perf-target/postgres, CARGO_INCREMENTAL=0, nice -n 10), needs linux/amd64.

| Run | Tree | Command | Result |
|---|---|---|---|
| r417, 05:29Z, cpus=2,mem=8g | main 1f4a8e88 | `bash scripts/check-production.sh` | exit 0, 10903: the baseline, before any change |
| 05:54Z, cpus=3,mem=6g | 11dee798, this branch's code | `cargo clippy -p amphora-pg-analysis -p amphora-pg-catalog --all-targets --locked -- -D warnings` | exit 0 |
| same | same | `cargo test -p amphora-pg-analysis --locked -- --test-threads=2` | 5 passed, 0 failed |
| this machine | this branch | `cargo fmt --all --check`, `python3 scripts/test_requirements.py`, `python3 scripts/vendor.py check postgres` | pass |

A macOS run of both crates' clippy and tests (r555) was stopped in the queue, 8 to 15 runs ahead of it.
