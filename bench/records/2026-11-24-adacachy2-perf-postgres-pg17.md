# Lane PG17 (postgres), 2026-11-24: unit 8's analysis, checked on macOS and not merged

Hour: 2026-11-24T06:01:31Z to 07:01:31Z (from the lane's first command). Branch `perf/postgres-pg17`
from `perf/postgres-pg16` 801938d1, with main fab12d7e merged in, then 8fc85651 (25538fc1). Not a speed
change: no A/B, nothing timed.

## Losses first

- Not merged. check-production has not run on this branch at all. It counts only on Linux, and Ada's
  benchmark window closed adacachy2 for the whole hour. Nothing of unit 8's tests or clippy has run on
  Linux since PG16's run at 05:54Z, which was before the sweep's fix.
- PG16's function sweep found a defect in PG16's own data. `pg_proc.pronargdefaults` was 0 for every
  function, so 37 functions resolved only with all their arguments written: `SELECT is_normalized('x')`
  and `SELECT make_interval(1)` were `42883` where 19 resolves them. Fixed in 9c324607 (below).
- 27 of the sweep's 14,382 calls are dropped, four of them answers (`plpgsql_validator` of int4, int8,
  regclass and unknown). They are nine calls each of three functions of one argument that initdb's scripts
  make (OIDs from 12000 on) and pg_proc.dat doesn't have: plpgsql_validator, and, inferred from the count
  and from 19's plpgsql and snowball scripts, plpgsql_inline_handler and dsnowball_init. Amphora has neither
  extension. A client that calls `plpgsql_validator` is told `42883`; a stock 19 resolves it.
- amphora-engine's focused tests ran on 9c324607 (main fab12d7e merged), not on 25538fc1.
- The sweep asks calls of one and two arguments only, as PG16 wrote it. A default is checked by it in 12
  of the 37 functions (those one or two arguments can call). The other 25 have the same source, and no
  server's answer.
- scripts/postgres-answers.sql's new filter (`oid < 12000`) has not run on a server. It drops the rows of
  the functions above and no others.

## What the sweep showed

- Red, on the branch as PG16 left it (ac1e61e5, run r601): `unknownin(unknown, -): 109, 19: Some("XX000")`,
  1 of 14,247. The test asked 14,247 calls where the server's script asked 14,382. 27 of the 135 missing are
  the calls dropped above. The other 108 are calls of functions whose defaults it read as 0.
- The cause of the 108: genbki.pl writes `pronargdefaults` 0 into postgres.bki, and bootstrap sets it to how
  many entries the row's `proargdefaults` has (`InsertOneProargdefaultsValue`,
  src/backend/bootstrap/bootstrap.c at REL_19_0, read from GitHub: it isn't vendored). 19's
  system_functions.sql declares no default, so `proargdefaults` is their only source. scripts/postgres-catalog.py
  copied the 0. It now counts the entries. data.rs changed in those 37 rows' `defaults` and nowhere else.
- `XX000` is the server's refusal of `unknownin(NULL)` after it chose OID 109: the column's unknown result has
  no conversion to text (`coerce_type`, parse_coerce.c). It joins `0A000`, `42804`, `42809` and `42P16` in the
  test's list of refusals that come after the choice.
- Green (9c324607, run r601): 14,355 calls asked, 3,336 of them answered other than `42883`, all as 19 answers.

- The elog is at the end of `coerce_type` at REL_19_0 (read from GitHub), and resolveTargetListUnknowns
  (parse_target.c), which runs after the function is chosen, calls it.

## Runs

macOS runs are on the MacBook Pro 2 (e30189ea, load average 42 to 77 of 18 cores, nice -n 10), named with `on`
as RULES rule 6 allows: six runs that named only their needs stayed 8 to 15 runs behind others at whichever
macOS machine they were asked of, from 06:02Z to the end of the hour, and the first of them waited at runner
2768d0d5, which the lane may not use, so its needs were narrowed to machines of 18 cores or more.

| Run | Tree | Command | Result |
|---|---|---|---|
| r601, 06:13Z, cpus=4,mem=8g | ac1e61e5 (PG16's unrun tests as it left them, on main fab12d7e) | `cargo test -p amphora-pg-analysis --locked --no-fail-fast` | the red run: 7 passed, 1 failed (the function sweep) |
| same | 9c324607 (the fix) | `cargo test -p amphora-pg-analysis -p amphora-pg-catalog --locked --no-fail-fast` | 8 and 7 passed, 0 failed |
| same | same | `cargo clippy -p amphora-pg-analysis -p amphora-pg-catalog --all-targets --locked -- -D warnings` | exit 0 |
| same | same | `cargo fmt --all --check` | exit 0 |
| r613, 06:18Z, cpus=4,mem=8g | same | the two crates' clippy with check-production.sh's 17 lints forbidden (`--no-deps -- -D warnings -F ...`) | exit 0. It is not check-production. |
| r602, 06:14Z, cpus=6,mem=16g | same | `cargo clippy -p amphora-engine --all-targets --locked -- -D warnings` | exit 0 in 7m40s, the log shows `Compiling amphora-engine` |
| same | same | `cargo test -p amphora-engine --lib --locked --no-fail-fast -- dialect:: vtab alter` | 35 passed, 0 failed (06:39Z, 17m13s of it building) |
| r684, 06:40Z, cpus=4,mem=8g | 25538fc1 (main 8fc85651 merged) | the two crates' tests, clippy and `cargo fmt --all --check` as in r601 | 8 and 7 passed, exit 0 each |
| r683, 06:40Z, cpus=6,mem=16g | same | `cargo clippy -p amphora-engine --all-targets --locked -- -D warnings` | exit 0 in 10m55s, the log shows `Checking amphora-engine` |
| same | same | `cargo test -p amphora-engine --lib ... dialect:: vtab alter` | not finished: stopped at 06:52Z while it was building. It passed at 9c324607 (r602); main's change to crates/engine came in between. |
| this machine | 25538fc1 | `cargo fmt --all --check`, `python3 scripts/test_requirements.py`, `python3 scripts/vendor.py check postgres` | pass |

25538fc1 differs from 9c324607 in one test's comment, records, and main's change to crates/engine.
