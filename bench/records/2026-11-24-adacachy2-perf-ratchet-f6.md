# Lane F6 (ratchet), 2026-11-24: check-production counts the engine as the shipped binary builds it

Ada's goal: a main whose gates mean the same on every machine and count what Amphora ships, with
ceilings in scripts/engine-ratchet.toml that never rise. The hour: first command 04:59:28Z, ends
05:59:28Z. Lane F5 was stopped by an interruption; its uncommitted tree (perf/ratchet-f5) was read,
kept, committed as found (f49bdd80) and measured by this lane.

## Losses first

- No test runs any of the new error returns: the suites below exercise the paths where the invariant holds.
  `commit_main_mvcc_tx_for_wal_session` has no caller in the repository (crates, vendor, bench), so nothing
  runs that edit at all.
- The gate counts the features that ship, not the profile that ships. It lints the dev profile, as it did
  before this lane; the binary ships as `--release`, where `debug_assertions` is off. 54 `cfg(debug_assertions)`
  and 3 `cfg(not(debug_assertions))` lines in the engine crates differ between the two. Measured at
  15dd57d8 (r452): the dev profile counts 10617, `--release` 10499. No site is in the release build alone, so
  nothing that ships goes uncounted; 118 counted sites are debug-only and don't ship (arithmetic_side_effects 21,
  cast_possible_truncation 15, disallowed_macros 35, indexing_slicing 27, panic 2, unreachable 3, unwrap_used 15).
  For Ada: should the gate count `--release` instead? The ceilings would fall by those 118.
- No ceiling rose. Every one fell or held, each now the count of the shipping build's features.
- README.md:47 says `cargo build --release` (no `-p`), and the workspace has no `default-members`, so
  it builds every workspace member, the engine with its own default features as well. The shipped build
  is the runbook's, `cargo build --release --locked -p amphora` (docs/runbook.md:12-13,
  scripts/cluster.sh:18). Not changed here. For Ada: should README.md:47 read `--locked -p amphora`?
- One shipping site the old gate never counted: crates/engine/storage/pager.rs:3334,
  `page.get().id() as u32`, cast_possible_truncation, compiled only with `autovacuum` off, as the shipped
  build has it. It is inside the new ceiling (519), not one of the 12, left as inherited code.
- Behaviour on impossible input changed from a panic to an error value (listed under "Changed" below);
  no caller relied on the panic (the one caller of `write_schema_version`, crates/amphora/src/db.rs:1268
  at 15dd57d8, returns its errors).

## What ships, and the engine features it turns on

| Binary | Where | Ships? | Engine features it turns on |
|---|---|---|---|
| `amphora` | crates/amphora/Cargo.toml `[[bin]] name = "amphora"` | yes: docs/runbook.md:12 "The `amphora` binary ... `cargo build --release --locked -p amphora`"; docs/runbook.md:25 installs `--bin target/release/amphora`; scripts/cluster.sh:18 "the amphora binary built for those machines (cargo build --release -p amphora)" | crates/amphora/Cargo.toml:24, `default-features = false`: `fs` (turns on amphora-engine-ext's `vfs`), `json`, `pure-rust-crypto` (aegis's `pure-rust`), `checksum`, `conn_raw_api` |
| `amphora-corpus` | crates/corpus/Cargo.toml `[[bin]] name = "amphora-corpus"` | no: "The competition corpus's runner (G2 steps 4 and 7)" | `fs`, `json`, `pure-rust-crypto`, `checksum`; not `conn_raw_api` (Cargo.toml:18) |
| `pg-oracle-digest` | crates/pg-oracle/src/bin | no: "test-only" | doesn't depend on the engine |

- Every crate has `publish = false` (Cargo.toml:51): no library ships either. No .github/workflows; the
  release path is the runbook and scripts/cluster.sh.
- Only crates/amphora and crates/corpus depend on amphora-engine, both `default-features = false`.
- Engine features that don't ship, and who turns them on: the engine's own defaults `uuid`, `time`,
  `series`, `encryption`, `percentile`, `autovacuum`, when the engine is built by itself (`-p amphora-engine`,
  `cargo test --workspace`); `simulator` (scripts/gates.sh:18, an engine test); `bench` and `io_uring`
  (bench/Cargo.toml, outside the workspace). Turned on by tests, simulators and benches, so they don't ship.

## The 12 sites

F4's count of the sites a `cargo clippy --workspace` build has and the engine-alone pass (`-p` the four
engine crates) lacks, from its saved JSON at 2571f841 (ws.json, engine-2571.json): 13 sites in the workspace
build only, and 1 in the engine-alone pass only, net 12.

| Lint | Workspace build only (all fixed; each compiled under `conn_raw_api`) | Engine-alone pass only |
|---|---|---|
| arithmetic_side_effects | mvcc/database/mod.rs:639 `MVTableId::from(-rootpage)`, :672 and :702 `let alternate_rootpage = -rootpage`, :2999 `if rootpage < 0 { -rootpage }` | |
| cast_possible_wrap | mvcc/database/mod.rs:660 and :2969 `rootpage as i64` | |
| disallowed_macros | connection.rs:3159 `turso_assert!`, :3597 `assert!`; mvcc/database/mod.rs:3020 `turso_assert!`; mvcc/portable_logical.rs:151 `assert!` | storage/wal.rs:5947 `turso_assert!` |
| disallowed_methods | mvcc/database/mod.rs:771 and :823 `then_some` | |
| indexing_slicing | mvcc/database/mod.rs:2841 `&log_record.buf[LOG_RECORD_PREFIX_SIZE..]` | |

- All 13 compile only with `conn_raw_api` (connection.rs's two also with `fs`); crates/amphora turns it on
  (crates/amphora/Cargo.toml:24), so they ship. All 13 are fixed in code below; none is left.
- Doesn't ship: storage/wal.rs:5947, `#[cfg(not(feature = "conn_raw_api"))]`, compiled only where
  `conn_raw_api` is off (the engine by itself, amphora-corpus by itself); neither ships.
- Evidence that each was one of the 13 and is gone: the shipping build at main (a64549cc) has 10630 sites and at
  F (50a488dc; 5237f16e adds only docs) 10617; the multiset difference by (lint, file, source line) is exactly
  the 13, with none added. After the fixes the workspace build has no site the engine-alone pass lacks (0).

## Changed

- crates/engine/connection.rs `write_schema_version`: a non-increasing schema version is
  `LimboError::InvalidArgument` (it panicked); `commit_main_mvcc_tx_for_wal_session`: a commit that ends
  without finalizing is `InternalError`.
- crates/engine/mvcc/database/mod.rs: `checked_neg` for `-rootpage`; a root page that doesn't fit `i64` is
  `Corrupt` (it wrapped); a log record shorter than its prefix is `InternalError` (a slice panic); a rejected
  object-map entry is `InternalError`; `then_some` became `if`/`else`.
- crates/engine/mvcc/portable_logical.rs: a repeated object-map id is `InternalError`.
- scripts/check-production.sh: the engine pass is `cargo clippy -p amphora` without `--no-deps`; the engine is
  linted as a dependency of what ships, with the features it resolves. CLAUDE.md and engine-ratchet.py say so.
- scripts/engine-ratchet.toml: every ceiling is the count of the shipping build's features, dev profile, at F; none raised (table).

| Lint | Main's ceiling | Ceiling now | The old gate's count at this commit |
|---|---|---|---|
| panic | 198 | 197 | 198 |
| unwrap_used | 448 | 433 | 448 |
| expect_used | 859 | 859 | 859 |
| unreachable | 458 | 458 | 458 |
| todo | 11 | 11 | 11 |
| unimplemented | 0 | 0 | 0 |
| indexing_slicing | 3182 | 3054 | 3182 |
| arithmetic_side_effects | 3345 | 3281 | 3345 |
| string_slice | 101 | 101 | 101 |
| cast_possible_truncation | 547 | 519 | 547 |
| cast_sign_loss | 249 | 234 | 249 |
| cast_possible_wrap | 685 | 674 | 685 |
| disallowed_macros | 780 | 756 | 780 |
| disallowed_methods | 40 | 40 | 40 |
| disallowed_types, dbg_macro, exit | 0 | 0 | 0 |
| total | 10903 | 10617 | 10903 |

"The old gate's count at this commit" is `-p amphora-engine ...` with the engine's default features (F4's way),
re-measured on this tree. Main's shipping build at a64549cc: 10630 (the 10617
left and the 13 this lane fixed). The old gate compiled none of those 13 (`conn_raw_api` off), nor `pager.rs:3334`
(`autovacuum` on).

## Checks (adacachy2, Rust 1.99.1, CARGO_HOME/CARGO_TARGET_DIR per RULES rule 6)

- r320: at F (50a488dc) check-production exit 0 in 56 s (10617, as the ratchet holds), then the shipping
  build (F and main a64549cc), workspace and engine-alone passes (JSON, the tables above): load 37-56.
- r367 at 5237f16e (F plus this record's doc edits; the engine's files as at F): check-production exit 0 in
  66 s with the engine's lib.rs files touched, so clippy re-linted the engine ("Compiling amphora-engine" in
  its log). Then a negative control: an `unwrap()` put in the shipping `conn_raw_api` function
  `write_schema_version` and reverted afterwards. check-production exit 1:
  "clippy::unwrap_used: 434, above the 433". The old gate compiles the engine without `conn_raw_api`,
  so it could not see it: inferred from its pass's JSON, which holds none of the 13 `conn_raw_api` sites; the
  control was not run against the old gate.
- r377 at 5237f16e: the engine's portable-logical tests,
  `cargo test -p amphora-engine --locked --features conn_raw_api --lib -- portable`: 28 passed, 0 failed.
- r388 at 5237f16e: the engine's whole `mvcc::` module, the same features: 558 passed, 0 failed, 12 ignored.
- r389 at 5237f16e: `cargo clippy --workspace --all-targets --locked -- -D warnings` (Linux): exit 0.
- r395 at the first merge of main, 2009772f (main moved in crates/amphora, crates/api and vendored hyper-raft,
  not in crates/engine): check-production exit 0 in 179 s, "Compiling amphora-engine" in both passes with the
  engine's lib.rs files touched again, 10617 as the ratchet holds; the same workspace clippy: exit 0 in 61 s.
  r397, same tree: `cargo test -p amphora --locked --test online_schema` (the caller of the shipping
  `write_schema_version` path, `restage_schema`): 10 passed, 0 failed.
- r416 at the second merge, e0947c86 (main moved inside crates/engine: dialect/postgres.rs, database.rs,
  translate, vdbe and others, 4 lines of mvcc/database/mod.rs, none of them this lane's edits): check-production
  exit 0 in 59 s, "Compiling amphora-engine" in both passes, 10617 as the ratchet holds (no lint's count
  changed with main's new engine code); workspace clippy `--all-targets -D warnings` exit 0 in 45 s;
  `cargo test -p amphora-engine --locked --features conn_raw_api --lib -- mvcc::`: 558 passed, 0 failed, 12 ignored.
- `cargo fmt --all --check`: exit 0 (this machine, 5237f16e and the merge).
- r446 at 15dd57d8 (main as landed): the engine's whole lib suite, `--features conn_raw_api --lib`, 2 threads,
  capped at 780 s: exit 124, NOT finished. 3250 tests; at the cap 3235 `... ok` lines (the suite's child test
  processes print some too), 31 ignored, 2 FAILED, 4 past 60 s each. The failures, neither in a file this lane
  changed and neither compared with main by this lane:
  `translate::optimizer::bushy_search_tests::tests::t_pl_8_13_a_left_join_inside_a_bushy_run_keeps_its_null_rows`
  (an earlier lane's record has it failing on main: 2026-11-23-adacachy2-perf-tpch.md:155 and :179), and
  `vdbe::morsel_scan::tests::t_qt_6_35_joins_over_rowid_ranges_run_on_morsels_as_the_serial_loop` (an earlier
  record has main 9fbfdbaf alone failing it, on macOS: 2026-11-24-macstudio2-perf-durable.md:45). Not shown by
  this lane: that either fails on main 15dd57d8 on this machine.
- r477 at 00c9534a (main ac2c429a merged: T21's translate/optimizer/lip.rs, V18's replicated.rs): check-production
  exit 0 in 58 s, 10617, the engine compiled in both passes. r489, same tree: workspace clippy
  `--all-targets -D warnings` exit 0 in 100 s.
- The last tree, 4a9a668e (main 219603df merged: lane Q8's database.rs, sqlite3_ondisk.rs, wal.rs): check-production
  exit 0 in 62 s, 10617, the engine compiled in both passes. This lane's delta over main there is this record and a
  comment in scripts/check-production.sh.
- The landed tree, 1a502992 (main c205dd7d merged: lane A21's replicated.rs, landed by lane A23): check-production
  exit 0 in 71 s, 10617, the engine compiled in both passes. Clippy `--all-targets` and the engine tests were not
  re-run on this last merge; main's only code in it is crates/amphora/src/database/replicated.rs.
- macOS run r351 (portable checks, no `on`) queued and never started; stopped at 05:36Z, unneeded. Its last
  candidate was runner 2768d0d5, which RULES rule 6 forbids: a macOS run without `on` can be placed there.

## Commands

```
# the gate's engine pass, the shipping build (scripts/check-production.sh does both passes)
CLIPPY_CONF_DIR="$PWD" cargo clippy -p amphora --locked --message-format=json -- -W clippy::<each of the 17 lints>
# the workspace and engine-alone passes this record compares against
CLIPPY_CONF_DIR="$PWD" cargo clippy --workspace --locked --no-deps --message-format=json -- -W ...
CLIPPY_CONF_DIR="$PWD" cargo clippy -p amphora-engine -p amphora-engine-parser -p amphora-engine-macros -p amphora-engine-ext --locked --no-deps --message-format=json -- -W ...
# sites: ~/.cache/amphora-perf/ratchet-out/f6-analyze.py, f6-diff2.py, f6-diff.py over {ship-f,ws-f,eng-f,ship-m}.json
```
