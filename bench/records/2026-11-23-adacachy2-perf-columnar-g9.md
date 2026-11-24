# Columnar lane G9 on adacachy2: decimal text and literals round correctly; arm (a) on main; no A/B yet (2026-11-23)

Ada's goal: Amphora about 10x faster than DuckDB on analytics, with answers identical to the serial
engine. TPC-H q01 and q06 run on one thread because they SUM or AVG over REAL, and Ada picks that
rule from an A/B of two arms. Lane G8 found the engine stores the literal `1e-300` one ulp below
its correctly rounded double, so a sum's answer could differ from SQLite's. This lane's units: (1)
make every decimal literal and text-to-REAL conversion correctly rounded, (2) build arm (a),
`RealSums::RowOrder`, (3) run the A/B of `RowOrder` against `Exact` on TPC-H SF1.

## Verdict, losses first

- **SQLite is not correctly rounded past 19 significant digits, so the engine now differs from
  it there.** On the lane's corpus (345,763 literals), SQLite 3.54.0 gives the correctly rounded
  double for all 126,434 literals of up to 17 significant digits and all 536 of 18 or 19. Of the
  218,793 with 20 or more, it misses 70,618 (32%), as a literal and as CAST text alike, each by an
  ulp or two (`3.539673131283986e+70` where the 96-character literal rounds to
  `3.5396731312839863e+70`). SQLite keeps about 19 digits and drops the rest. The engine now
  rounds every digit, as asked. Data with at most 19 significant digits, TPC-H's prices and
  discounts included, reads the same as SQLite's. Ada's choice is below.
- **The A/B has no numbers.** Unit 3 did not start: the hour went to units 1 and 2 and their
  checks, and kiln refused room on adacachy2 seven times (0 to 3 of 32 cores idle, load 59 to
  97). The release bench binary of 62110f56 (both arms in one binary) was building at the
  hand-over (r1173, into `~/.cache/amphora-perf-bin/columnar/62110f56`).
- **Arm (a) holds every SUM, AVG and TOTAL input until the end.** That is its memory cost, not yet
  measured (unit 2 below).
- **One known flake showed once.** On f668352e, before the merge with main, t_qt_6_30 failed at
  its "the hook didn't run" assertion (line 2971), as on main in G7's and G8's runs (r1157). It
  passed on the merged head (r1172).
- **The release bench build did not finish.** r1112 built main 7001bf5d, not the lane's head:
  in the worktree, `git checkout --detach FETCH_HEAD` read the worktree's own empty FETCH_HEAD.
  It was stopped to give the merge check room. It compiled part of the release dependencies
  into the lane's target dir.

## Unit 1: correctly rounded decimal text

Both decimal-to-double converters scaled at most 19 significant digits by powers of ten in
double-double arithmetic (Dekker), the algorithm of SQLite's sqlite3AtoF:

- `numeric::str_to_f64`: literals (`parse_numeric_literal`), CAST, numeric text (`Numeric::from`).
- `vdbe::affinity::try_for_float`: a column's numeric affinity.

That rounding is not correct near a halfway point, and it drops digits past the 19th. Each
converter now keeps its own grammar and classification and takes the value from Rust's parse of
the number's text, which rounds every digit to nearest, ties to even. `DoubleDouble` was left
with no use and is deleted. Two behaviours stay as they were: `try_for_float` answers +0.0 for a
zero mantissa (`-0.0` included), and `str_to_f64` keeps its prefix and fraction classes.

Tests (`crates/engine/numeric/decimal_tests.rs`):

- `g9_decimal_text_converts_to_the_correctly_rounded_double` builds 345,763 literals from a
  seeded generator and holds both converters bit-equal to Rust's parse. The literals are:
  - random doubles printed shortest, at 17 digits and at 26 digits;
  - the exact halfway point between a double and the next one up, and that point plus and minus
    one part in 10^21 (subnormals one in four);
  - mantissas of 1 to 800 digits with exponents from -1000 to 399;
  - the literal forms: signs, `E`, leading zeros, exponent zeros, `.5`, `5.`;
  - 31 edge literals: overflow, the smallest subnormals, 2^53 + 1, 2^64.
- `g9_prefix_texts_take_the_correctly_rounded_double_of_their_number` checks texts with trailing
  junk.
- `g9_sql_literals_casts_and_real_columns_hold_the_correctly_rounded_double` runs every 97th
  literal through `SELECT <literal>`, `CAST('<literal>' AS REAL)` and text into a REAL column.
- `AMPHORA_DECIMAL_CORPUS=<file>` writes the corpus. `python3 -I bench/decimal_sqlite.py <file>`
  then counts SQLite's answers against Python's correctly rounded `float`.

| run | head | what | result |
|---|---|---|---|
| r1111 | 3e48462b | `cargo clippy -p amphora-engine --all-targets --locked -- -D warnings` | clean (40 s) |
| r1111 | 3e48462b | `cargo test -p amphora-engine --lib -- g9_ numeric:: vdbe::affinity util::tests` | 123 passed, 0 failed (84 s) |
| r1132 | 81a296c5 | `bench/decimal_sqlite.py` on r1111's corpus, SQLite 3.54.0 | 70,618 of 345,763 wrong, all with 20+ significant digits |
| r1145 | 8e135edc (merged with main 1df94b59) | the same clippy and tests | clean (48 s); 123 passed, 0 failed (52 s) |
| r1145 | 4e3e91ca (main 378fe9ed with only the new tests; branch perf/columnar-g9-before, not for main) | `cargo test -p amphora-engine --lib -- g9_` | 3 of 3 fail: 229,752 wrong conversions over the 345,763 literals and the two converters; `SELECT 4.7712825755521808e-141` answered 4.7712825755521814e-141 |

Before the fix the engine missed even 17-digit literals, where SQLite is right: the before arm's
SQL test stopped at `4.7712825755521808e-141`. The corpus test's message counts conversions (two
per literal), not literals.

## Ada's choice: long literals

- **Correctly rounded (on main now).** Every literal and text becomes the nearest double. With
  at most 19 significant digits, the answer is SQLite's. Past that, about a third of literals
  read an ulp or two away from SQLite's.
- **SQLite's digits.** Keep only the first 19 significant digits, as SQLite does. Literals of
  20+ digits would then read as SQLite reads them, which is not the nearest double.

## Unit 2: arm (a), `RowOrder`, on main with the default unchanged

- The switch. `RealSums::RowOrder` (`crates/engine/connection.rs`), and
  `AMPHORA_REAL_SUMS=roworder` in `amphora-bench --planner`. The default stays `Serial`.
- Workers (`vdbe/morsel.rs`). `Accumulator::Ordered { func, inputs: Vec<(usize, Value)> }` holds
  SUM, AVG and TOTAL under `RowOrder`. Its `step` keeps each non-NULL argument with its
  `position`, the global row position `fold` already passes and `Group::first_row` uses, and
  charges the statement's memory budget. Its `merge` appends. Its `finish` sorts by position and
  replays the serial loop's own code on one payload: `init_agg_payload`, `update_agg_payload` for
  each input, `finalize_agg_payload` (made `pub(crate)` in `vdbe/execute.rs`). The bits are then
  the serial loop's at every worker count, if its order within a group is row order.
- `morsel_scan::no_real_outputs` drops the sum inputs under `RowOrder` as under `Exact`, so
  q01's and q06's shapes stay on the workers.
- The test, `g9_row_order_real_sums_answer_as_today_at_every_worker_count`, runs G8's data and
  statements (grouped and not) at 1, 2, 4, 8 and 16 stated cores against `Serial`. A grouped
  statement passing it shows the serial loop steps a group's rows in row order. A sorter that
  reorders equal keys would fail it.
- A gap the test shares with G8's helper: an error in `finish` sends the statement back to the
  serial loop, and the helper's row count still passes.

| run | head | what | result |
|---|---|---|---|
| r1157 | f668352e | clippy, then `g9_ g8_ audit_3_p2 vdbe::morsel:: sum_ avg_ total_` | clean (36 s); 60 passed, 1 failed (t_qt_6_30, the known flake) |
| r1172 | 62110f56 (unit 1 and main merged) | clippy, then those and `numeric:: vdbe::affinity util::tests` | clean (29 s); 181 passed, 0 failed (63 s), `g9_row_order_...` among them |

The grouped statements pass at every core count, so the serial loop steps a group's rows in row
order for these shapes.
- The cost to measure: every input is held until the end, then folded on one thread. q01 has 7
  REAL aggregates over 5.9 M rows, about 41 M inputs; at 16 bytes or more each (inferred, not
  measured) that is 0.7 GB or more. G7's sequencer, which folds each morsel's run as soon as the
  runs before it are in, holds only the runs that arrive early.

## Successor: the A/B (unit 3)

On adacachy2 (where the earlier numbers are), `needs linux/amd64,cpus=12,mem=48g`, from the one
binary `~/.cache/amphora-perf-bin/columnar/62110f56` (r1173; if it is missing, rebuild as r1173
does), so both arms share one calibration (RULES 5e: one executable, one key):

1. Alternate the arms 5 times: `AMPHORA_REAL_SUMS=roworder`, then `=exact`, each `amphora-bench
   --planner --bench tpch --sf 1 --targets amphora,duckdb-mt --only q01,q06,q09 --cores
   1,2,4,8,12 --runs 1 --answers <dir per arm and round>`.
2. Beside each run, record peak RSS (`VmHWM` from `/proc/<pid>/status`), the load average, and
   CPU and I/O pressure (`/proc/pressure/{cpu,io}`). G6's `scale.sh` and `summ.py` under
   `~/.cache/amphora-g6` do most of it.
3. Count drift against SQLite with `python3 -I bench/planner/answers.py <dir>`, and count bit
   differences from the tsv files too, since answers.py compares REALs within 1e-9.
4. q09 stays serial under both arms: a join still refuses REAL columns at plan time. Say so in
   the table.

## Successor: commands

The checks above, on adacachy2, `needs linux/amd64,cpus=4,mem=16g`, `sync false`, in
`~/.cache/amphora-perf/columnar-g8-remote`, with `CARGO_HOME=$HOME/.cache/cargo-home-perf-columnar
CARGO_NET_OFFLINE=true CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/columnar
CARGO_BUILD_JOBS=4 CARGO_INCREMENTAL=0`. Outputs are in `~/.cache/amphora-g9/` (clippy-, tests-,
corpus.txt, sqlite-check.txt). For a worktree, fetch inside it or check out
`origin/<branch>`; FETCH_HEAD is per worktree.
