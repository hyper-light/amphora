# Columnar lane G8 on adacachy2: the SUM-over-REAL switch with arm (b) on main, default unchanged; arm (a) and the A/B not built (2026-11-23)

Ada's goal: Amphora about 10x faster than DuckDB on analytics, with answers identical to the serial
engine. TPC-H q01 and q06 still run on one thread because they SUM or AVG over REAL. Ada decides
the SUM-over-REAL rule by an A/B of (a) a row-order compensated sum (KBN), bit-identical to today's
serial KBN at every worker count, and (b) an exact sum everywhere, rounded once, the serial path
included. This lane was to build both arms behind one switch, measure them on TPC-H SF1, and merge
the switch with today's behaviour as the default.

## Verdict, losses first

- **Merged to main: the switch and arm (b), default unchanged (`Serial`).** Gate on fb8d0710
  (r1035, adacachy2, load 55 to 77 on 32 cores): `cargo fmt` clean; `cargo clippy -p
  amphora-engine --all-targets --locked -D warnings` clean; `cargo test -p amphora-engine --lib`
  over `g8_ audit_3_p2 vdbe::morsel:: vdbe::execute::tests agg window sum_ avg_ total_`, 138
  passed and 3 failed. The failures are t_qt_6_31 and 6_32, which panic at builder.rs:2131 as main
  227d65ee does in the same run, and t_qt_6_30 at its "the hook didn't run" assertion (line 2971;
  its answers matched). 6_30 passed on main in that run. G7 traced this flake to per-executable
  calibration, and lane T5 is pinning cost versions. Main moved only in docs after r1035.
- **The first hard-case test was wrong, not the sum.** On 9336a4d6 (r997), both paths answered
  9.999999999999999e-301 for 1e300 + 1e-300 - 1e300, where the test expected Rust's 1e-300. The
  engine stores the literal `1e-300` one ulp lower than Rust parses it. The oracle now takes the
  stored values (fb8d0710).
- **Arm (a), row-order KBN on the workers, is not built.** The switch has no `RowOrder` value yet.
  G7's design for it, checked against the code, is unchanged (G7's record, "Arm (a)").
- **No A/B numbers.** Check runs were refused for room from 15:22Z to 15:27Z and again at
  15:33Z, and the release bench build never started. No TPC-H timing, peak memory or
  answer-drift figure exists for either arm.
- **Arm (b) gaps, by design for this hour:**
  - A join still refuses REAL columns at plan time, so q09 stays on the serial loop under both
    arms. Under `Exact`, q09's serial loop sums exactly; the workers do not take it.
  - `ExactSum` (Shewchuk partials, pre-existing) is order-dependent when a running sum of finite
    values passes the double's range (about 1.8e308): it records an infinity in one order and
    not in another. Inside the range it is exact and order-free. Today's parallel SUM path has
    the same limit. A fix is a superaccumulator (a fixed-point array over the whole exponent
    range); it was not attempted.

## What landed

The switch is `Connection::set_real_sums(RealSums)` (`crates/engine/connection.rs`), read when a
statement runs, so one prepared statement can be timed under either arm. `RealSums::Serial` is
today's behaviour and the default. Ada's decision then means setting the default and deleting the
losing arm.

`RealSums::Exact` (arm b):

- Workers (`vdbe/morsel.rs`). `Accumulator::Mean` (AVG, TOTAL) takes REALs into an `ExactSum`.
  Its integers join the sum as exact pieces (`exact_total`, the code SUM already used). The
  2^53 refusal does not apply under `Exact`, because the serial loop is exact too.
  `morsel_scan::no_real_outputs` drops SUM, AVG and TOTAL inputs from the outputs where a REAL
  sends the statement back. q01's and q06's shapes then stay on the workers.
- Serial loop (`vdbe/execute.rs`). A SUM, TOTAL or AVG payload is one slot longer than today's
  (the marker), and `op_agg_step_slow` makes it at a group's first row. While a SUM stays an
  exact INTEGER it runs as today, fast path included. From the first REAL, or the first integer
  overflow, slot 0 holds the exact sum's bytes (`ExactSum::write_to`). The fast paths take only
  numbers there, so every later step goes to `exact_sum_step` or `exact_avg_step`. SQLite's
  sumStep semantics are kept:
  - an overflow is reported at the end unless a REAL clears it;
  - both infinities give NULL, which stays;
  - `+0.0` joins the sum, so an all-zero sum is `+0.0`;
  - AVG is the exact sum, rounded once, divided by the count.
  
  A window frame's leaving row is subtracted exactly (`exact_inverse`).
- Bench. `AMPHORA_REAL_SUMS=exact` (or `serial`, the default) sets the arm on `amphora-bench
  --planner`'s Amphora connections. Both arms then run from one binary and plan from one
  calibration (RULES 5e).

Tests (`vdbe/morsel_scan_tests.rs`):

- `g8_exact_real_sums_answer_as_the_serial_loop_at_every_worker_count` covers SUM, AVG and
  TOTAL over REALs, INTEGERs and both mixed: prices, 1e-300 to 1e300, 5e-324, -0.0, integers
  past 2^53 and near -2^62, grouped and not. It holds them bit-equal (`{:?}`) to the serial loop
  at 1, 2, 4, 8 and 16 stated cores, with the copied body run over every row each time.
- `g8_exact_real_sums_hold_the_hard_cases` covers cancellation (1e16 + 1 - 1e16 = 1), tenths
  (0.1 + 0.2 - 0.3 = 2^-55 exactly), huge with tiny (1e300 + 1e-300 - 1e300 = 1e-300), seven
  smallest subnormals (bits 7), two -0.0 (+0.0), +inf, -inf, both infinities (NULL for all
  three) and 2^53 + 1 + 1. Each case runs on the serial loop and on the workers at every count.

## Successor: commands

The check (RULES 5b gate), on adacachy2, `needs linux/amd64,cpus=8,mem=24g`, sync false:

```
R=$HOME/.cache/amphora-perf/columnar-g8-remote
[ -d "$R/.git" ] || git clone -q git@github.com:hyper-light/amphora.git "$R"
cd "$R" && git fetch -q origin perf/columnar-g8 && git checkout -q --detach origin/perf/columnar-g8
export CARGO_HOME=$HOME/.cache/cargo-home-perf-columnar CARGO_NET_OFFLINE=true \
  CARGO_TARGET_DIR=$HOME/.cache/amphora-perf-target/columnar CARGO_BUILD_JOBS=8 CARGO_INCREMENTAL=0
nice -n 10 cargo clippy -p amphora-engine --all-targets --locked -- -D warnings
nice -n 10 cargo test -p amphora-engine --lib --locked -- --test-threads=8 g8_ audit_3_p2 \
  vdbe::morsel:: vdbe::execute::tests agg window sum_ avg_ total_ --skip t_qt_6_18 --skip t_pl_8_14
```

Known failures on main in that filter: t_qt_6_24, 6_29, 6_30, 6_31, 6_32, 6_33 and 6_35. G7's
record has them, and 6_29 and 6_30 move with per-executable calibration (lane T5).

The A/B (unit 2), after the gate:

1. Build: `cp Cargo.lock bench/Cargo.lock && cargo build --release --manifest-path
   bench/Cargo.toml`, then copy the binary to `~/.cache/amphora-perf-bin/columnar/<sha>`.
2. Run timed runs with needs `linux/amd64,cpus=12,mem=48g`. Use one `XDG_CACHE_HOME` for both
   arms and one binary.
3. Run `amphora-bench --planner --bench tpch --sf 1 --targets amphora,duckdb-mt --only
   q01,q06,q09 --cores 1,2,4,8,12 --runs 5 --answers <dir>`. Alternate the arms 5 times
   (`AMPHORA_REAL_SUMS=serial`, then `=exact`). Record peak RSS (`VmHWM`), load, and CPU and I/O
   pressure beside each run.
4. Count drift against SQLite with `python3 -I bench/planner/answers.py <dir>`. It compares REALs
   within 1e-9, so count the bit differences from the tsv files too.

## Files

- Branch `perf/columnar-g8`: bec8f463 (switch and arm b), 840e7443 (window frames), f53fe695
  (a borrow fix), this record, and the merge of main ea04b5b7.
- On adacachy2 in `~/.cache/amphora-g8/`: `fmt-`, `clippy-` and `tests-` for 9336a4d6 (r997) and
  fb8d0710 (r1035), and `tests-main-227d65ee.txt`.
