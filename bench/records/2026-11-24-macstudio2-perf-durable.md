# T1 reviewer landing (lane D4, 2026-11-24)

This landing carries the T1 reviewer's fixes onto main. It takes `origin/t1/main-review` at `3b8620fa`, the branch's head when lane D4 started (02:57Z), and main at `9fbfdbaf`. The branch's two later commits are left for the next landing, except `f02b5e67`, a test fix this landing needs (below).

## Commits on top of lane D2's merged tree (`e966cfbe`)

| Commit | What it does |
|---|---|
| `0f1f3f13` | Merges `60cdd988` (AC-MB.1.1, a Linux-only memory test; no conflict) and `3b8620fa` (T-T1.10.W9–W11). The lost-reply test now drives main's ringing: its held resolution goes through `step(..., step_answer(&first), ...)`, which calls `Database::call_internal_ringing`. |
| `d96e278d` | Merges `origin/main` at `7de5344d`. No conflicts. |
| `29b4b65f` | `cargo fmt --all` over five files from the branch. |
| `ac97d56d` | Restores `eb7fbc1d`'s guard in `shape_chain`. `0c4a4587` kept its regression test but not the guard, and main had no fix of its own. Without it a forced merge join in a kept-side outer join chain compiled to a `MorselAggregate` while `EXPLAIN` said `MERGE JOIN`. |
| `8b798818` | Cherry-picks `f02b5e67`. `e966cfbe` kept main's Linux-only `Descriptors::waiting()` beside the reviewer's portable test, so the amphora lib tests did not compile on macOS (E0599). |
| `689da68f` | Restores main's four lint allowances in `examples/admission.rs`. `e43ede95` dropped them while main's later code in that file still unwraps, which gave 22 clippy errors. |
| `ac2c48c5` | Merges `origin/main` at `9fbfdbaf` (lane R9's bound parameter in the column copy's filter). No conflicts; `shape_chain` keeps the guard. |

## Run r36 (adacachy2, `e966cfbe`)

Only the engine lib step ran before the run stopped itself. Two tests failed: `a_kept_side_merge_join_stays_serial` (cause and fix above) and `t_qt_6_35_joins_over_rowid_ranges_run_on_morsels_as_the_serial_loop`. `t_qt_6_18_join_workers_run_a_copy_of_the_loop_body_as_the_serial_loop` ran 13 minutes without finishing. The run ended before any panic message printed.

## Checks on the landing tree

macOS on Mac Studio 2 with 8 cores, load 1 to 10. The landing column is run r149 on `ac2c48c5`, after main moved into `crates/engine`; runs r116, r117, r129 and r130 on `ac97d56d` to `689da68f` gave the same results. Main alone ran on the same machine (run r118, main `7de5344d`).

| Check | Landing | Main `7de5344d` alone |
|---|---|---|
| amphora lib: `xgroup::` (W4, W5, W9 lost reply, W13), `database::across::`, `transport::`, `db::tests::busy`, `memory::`, `descriptors::` | 97 passed, 2 ignored | not run |
| `--test transport` | 9 passed | not run |
| engine lib: `skipping::`, `storage::versioned`, `morsel_scan`, `translate::parallel`, `busy::` | 306 passed, 1 failed (`t_qt_6_35`) | `morsel_scan`: 38 passed, 1 failed (`t_qt_6_35`, same message) |
| `--test adaptive_joins --features simulator` | 23 passed | not run |
| `--test join_methods` | 4 passed, 1 ignored; `pl_5_star_join_filters_fire_and_answers_hold` passes | 4 passed, 1 ignored |
| `t_qt_6_18` alone | did not finish in 20 minutes; stopped | did not finish in 24 minutes; stopped |
| `cargo clippy --workspace --all-targets -D warnings` | exit 0 | not run |
| `cargo check --manifest-path bench/Cargo.toml --all-targets` | exit 0; not `--locked`, since `bench/Cargo.lock` is not tracked | not run |
| `scripts/check-production.sh` | The forbid pass over every crate but the engine passed (run r117). The engine ratchet did not run: python3 there lacks `tomllib`, and the ratchet has Linux limits only. | not run |

Linux on adacachy2 with 4 cores, load 35 to 60 (run r155 on `ac2c48c5`):

| Check | Landing |
|---|---|
| `scripts/check-production.sh` | exit 0 in 80 s; checked `amphora`, `amphora-api`, `amphora-corpus` and `amphora-engine-parser`, compiled `amphora-engine`, `amphora-engine-ext` and `amphora-engine-macros`; "engine-ratchet: 10903 left in the engine's production code across 17 lints, as the ratchet holds" |
| amphora lib: `memory::` (with `60cdd988`'s Linux-only test), `xgroup::`, `transport::`, `descriptors::` | 94 passed, 1 ignored |
| `--test transport` | 9 passed |

## Reds that are main's own

- `t_qt_6_35_joins_over_rowid_ranges_run_on_morsels_as_the_serial_loop` fails on main alone with the landing's message: "SELECT count(*), sum(d.b) FROM f JOIN d ON f.a = d.a WHERE f.id > 2000 on 1 cores: the hook built no link".
- `t_qt_6_18_join_workers_run_a_copy_of_the_loop_body_as_the_serial_loop` did not finish in 24 minutes on main alone on Mac Studio 2. On the landing it did not finish in 20 minutes there, nor in 13 minutes on adacachy2 in r36.
