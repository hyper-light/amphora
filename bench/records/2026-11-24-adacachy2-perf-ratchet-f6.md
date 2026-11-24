# Lane F6 (ratchet), 2026-11-24: check-production counts the engine as the shipped binary builds it

Ada's goal: a main whose gates mean the same on every machine and count what Amphora ships, with
ceilings in scripts/engine-ratchet.toml that never rise. The hour: first command 04:59:28Z, ends
05:59:28Z. Lane F5 was stopped by an interruption; its uncommitted tree (perf/ratchet-f5) was read,
kept, committed as found (f49bdd80) and measured by this lane.

## Losses first

- Nothing rose. Every ceiling fell or held, each now the count of the shipping build.
- README.md:47 says `cargo build --release` (no `-p`), and the workspace has no `default-members`, so
  it builds every workspace member, the engine with its own default features as well. The shipped build
  is the runbook's, `cargo build --release --locked -p amphora` (docs/runbook.md:12-13,
  scripts/cluster.sh:18). Not changed here. For Ada: should README.md:47 read `--locked -p amphora`?
- One shipping site the old gate never counted: crates/engine/storage/pager.rs:3334,
  `page.get().id() as u32`, cast_possible_truncation, compiled only with `autovacuum` off, as the shipped
  build has it. It is inside the new ceiling (519), not one of the 12, left as inherited code.
- Behaviour on impossible input changed from a panic to an error value (listed under "Changed" below);
  no caller relied on the panic (the one caller of `write_schema_version`, crates/amphora/src/db.rs:1258,
  returns its errors).

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
- Doesn't ship: storage/wal.rs:5947, `#[cfg(not(feature = "conn_raw_api"))]`, compiled only in builds of the
  engine alone (its tests' builds).
- Evidence that each was one of the 13 and is gone: the shipping build at main (a64549cc) has 10630 sites and at
  this commit (F, the tree of 5237f16e) 10617; the multiset difference by (lint, file, source line) is exactly
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
- scripts/engine-ratchet.toml: every ceiling is the shipping build's count at this commit; none raised (table).

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
re-measured on this tree. Main's shipping build at a64549cc: 10630 (the 13 sites, then counted by neither gate,
and `pager.rs:3334`).

## Checks (adacachy2, Rust 1.99.1, CARGO_HOME/CARGO_TARGET_DIR per RULES rule 6)

@@RUNS@@

## Commands

```
# the gate's engine pass, the shipping build (scripts/check-production.sh does both passes)
CLIPPY_CONF_DIR="$PWD" cargo clippy -p amphora --locked --message-format=json -- -W clippy::<each of the 17 lints>
# the workspace and engine-alone passes this record compares against
CLIPPY_CONF_DIR="$PWD" cargo clippy --workspace --locked --no-deps --message-format=json -- -W ...
CLIPPY_CONF_DIR="$PWD" cargo clippy -p amphora-engine -p amphora-engine-parser -p amphora-engine-macros -p amphora-engine-ext --locked --no-deps --message-format=json -- -W ...
# sites: ~/.cache/amphora-perf/ratchet-out/f6-analyze.py, f6-diff2.py, f6-diff.py over {ship-f,ws-f,eng-f,ship-m}.json
```
