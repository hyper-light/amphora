# Numeric lane G16 on adacachy2: decimal_sum as a window function (2026-11-23)

Ada: REALs follow https://www.sqlite.org/floatingpoint.html exactly; where sqlite3 3.54 differs
from the page, the page wins; where the page is silent, sqlite3 3.54.0 decides. Deliverable:
decimal_sum works as a window function exactly as 3.54.0's does. Started 22:09Z from main 169b8ba1.

## Verdict, gaps first

- **3.54.0 differs where the page decides (by the coordinator, under the page-wins rule).**
  floatingpoint.html 2.2.5: decimal_sum is "an aggregate, like the built-in total() aggregate
  function"; total() of no rows is 0.0. So decimal_sum of no rows is `0`, and so is an empty
  window frame nothing was ever added to: `decimal_sum(x) OVER (ROWS BETWEEN 1 PRECEDING AND 1
  PRECEDING)` over '1.5','2' is `0|1.5` in Amphora, `NULL|1.5` in 3.54.0 (`total()` there: 0.0).
- **Not changed:** the error behind "out of memory" is still `ExtensionError`, not SQLITE_NOMEM
  (G15's open item). `median`, `percentile*` and `stddev` (percentile.rs) still refuse every window,
  as before; not checked against 3.54.0.
- **Not run here:** scripts/check-production.sh (the coordinator's gate). The change removes
  inherited hits (old step/finalize indexing and casts, compute_external's panic!) and adds none
  that I know of, so a count may fall below scripts/engine-ratchet.toml.

## Before

The planner refused any extension aggregate under OVER, frame or not (`link_with_window`:
"decimal_sum() is an extension aggregate and cannot be used as a window function"). Read from the
code; the same branch is what `native_extension_tests` holds for a native aggregate
(`weighted(3, 7) OVER ()`). AggFunc had step and finalize only; AggInverse and AggValue had no
extension path (`unreachable!` / finalize, which consumes the state).

## What changed

| piece | change |
|---|---|
| `AggFunc::step` | returns `Result<(), Error>`: an Err fails the statement at that row (sqlite3_result_error in an xStep). percentile.rs's five steps return `Ok(())`, behaviour unchanged |
| `WindowAggFunc` (new) | `inverse` and `value` (sqlite3_create_window_function's xInverse, xValue); derived by `#[derive(AggregateDerive)] #[window]` |
| ExtensionApi | `register_window_function` appended (both hooks or none: `Option<WindowCallbacks>` on `ExtFunc::Aggregate`) |
| planner | an extension aggregate with both hooks goes into any frame; others are refused as before |
| VDBE | AggInverse and AggValue call the hooks; AggFinal still finalizes |
| ExternalAggState | owns its state: dropped unfinished (a window's partition reset, an abandoned statement, a failed step) it runs finalize, which frees it, as SQLite's release of an aggregate register runs xFinalize. Not Clone; dead `compute_external` gone |
| decimal_sum | decimalSumInverse ported; a row decimal_new cannot read fails at that row; a sum past 10,000,000 digits stays marked until value/finalize, as decimal_add marks it; no rows `0` |

## Expected values (sqlite3 3.54.0 2026-10-09, `sqlite3 -batch :memory:`) and Amphora

Rows '1.5','2','3' unless said.

| query | 3.54.0 | Amphora (b81423ae) |
|---|---|---|
| `ROWS BETWEEN 1 PRECEDING AND CURRENT ROW` | `1.5\|3.5\|5.0` | same |
| `ROWS BETWEEN 1 FOLLOWING AND 1 FOLLOWING` | `2.0\|3.0\|0.0` | same |
| `ROWS BETWEEN 2 FOLLOWING AND 3 FOLLOWING` | `3.0\|0.0\|0.0` | same |
| `OVER ()` | `6.5\|6.5\|6.5` | same |
| `OVER (ORDER BY x)` | `1.5\|3.5\|6.5` | same |
| `CURRENT ROW AND CURRENT ROW` over '0.25','-1','1' | `0.25\|-1.00\|1.00` | same |
| `1 PRECEDING AND CURRENT ROW` over NULL,'2',NULL | `0\|2\|2` | same |
| `1 PRECEDING AND 1 PRECEDING` over '1.5','2' | `NULL\|1.5` | `0\|1.5` (page wins) |
| `decimal_sum(x), total(x)` of no rows | `NULL\|0.0` | `0\|0.0` (page wins) |
| rows '1','12e9999999', then `abs(-9223372036854775808)` | out of memory | same |
| rows '1e9999999','1', then `abs(-9223372036854775808)` | integer overflow | same |

The last two rows tell "at that row" from "at the end": 3.54.0 stops at the unreadable second row
before the third row's overflow; a sum that only outgrows the limit is reported at the end, so there
the third row's overflow comes first.

## Tests

crates/engine/tests/floating_point_doc.rs: `g16_decimal_sum_of_no_rows_is_zero_like_total`,
`g16_decimal_sum_over_window_frames_gives_3_54_0s_answers`, `g16_decimal_sum_fails_at_the_bad_row`;
decimal.rs `g15_nulls_follow_decimal_c_per_function` now expects `0` for no rows (2.2.5).

## Checks

adacachy2 (`on` 00aad8daff2fa3c9daa65d53c425d450, sync false, `~/.cache/amphora-perf/numeric-remote`),
`CARGO_HOME=~/.cache/cargo-home-perf-numeric CARGO_NET_OFFLINE=true
CARGO_TARGET_DIR=~/.cache/amphora-perf-target/numeric CARGO_INCREMENTAL=0`, cpus=8, mem=24g.

| run | head | result |
|---|---|---|
| r1682 | b81423ae | clippy failed: `new_aggregate` 8 arguments, one test caller not updated (fixed in 9322622b); lib tests did not build for the same reason; `floating_point_doc` 51 passed (the 3 G16 tests included) |
| r1694 | d04b54da (9322622b merged with main f91a9a92) | clippy: percentile.rs's test driver dropped a step's Result (fixed in 9ceb70f9); lib `decimal:: native_extension percentile external_aggregate window` 72 passed; `floating_point_doc` 51 passed |
| r1704, r1710 | 9ceb70f9 | cpus=8 and cpus=4 refused (2 of 32 cores idle); at cpus=2,mem=8g (jobs 2) clippy clean, load 75 |

    cargo clippy -p amphora-engine -p amphora-engine-ext -p amphora-engine-macros --all-targets --features amphora-engine/bench --locked -- -D warnings
    cargo test -p amphora-engine --features bench --locked --lib -- decimal:: native_extension percentile external_aggregate window
    cargo test -p amphora-engine --features bench --locked --test floating_point_doc
