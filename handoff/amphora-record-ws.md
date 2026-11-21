# amphora-record-ws handoff (goal 9c63f7d4, CP-T1.8; 2026-11-12)

Agent amphora-record-ws, started by the Amphora agent (kiln 01a1ce3a7f547793aa239248162c3d3f). Nothing pushed, nothing merged.

## Branch

- Worktree ~/Projects/amphora-t1-record-ws, branch t1/record-ws from the batch head b151af0.
- Head **33438fd57f7285456862ef750754962223886ffe**, tree **fad3cca0089d08d5126be9657735144be74b3d82**. One commit,
  signed through kiln's signing (x509, kiln-git-sign; git can't check it locally, kiln does).
- No production code. docs/design/replication.md untouched (amphora-g-slot's to write).

## Files brought from t1/write-starve 044c44f, byte-identical (`git diff 044c44f HEAD -- <files>` is empty)

- docs/research/follower-write-starvation.md
- bench/records/2026-11-07-studio-local2-write-starve-traces.md and .jsonl.gz
- bench/records/2026-11-07-write-starve-model.md and .jsonl.gz
- bench/records/2026-11-09-write-starve-driven.md
- bench/records/2026-11-09-write-starve-kiln-leg.md
- bench/write_starve_table.py
- crates/amphora/tests/write_starve_model.rs: std only, needs none of the prototypes. On studio local-2 (heavy lock
  absent), `cargo test --locked -j 8 -p amphora --test write_starve_model`: 3 passed, 1 ignored (the_ab_sweep, release
  with AMPHORA_TRACES/AMPHORA_MODEL_OUT). `cargo fmt --all --check` passes.

## New: bench/records/2026-11-12-write-starve-release-ab.md

kiln's promotion test in release, runner cec37f60 (label adas-mac-studio.local, hostname Adas-Mac-Studio-3.local; Apple
M4 Max, 14 cores, 36 GiB, macOS 26.2 25C56), kiln scratch 7ad48c5, script probe-carrier ws-ab.sh (6705c38, 43b4534).
Set 1 ~/.cache/ws-ab (5 rounds, 03:57-04:31Z, start loads not kept), set 2 ~/.cache/ws-ab-2 (3 rounds, 05:08-05:28Z,
loads 2.30-3.59, killed at round 4). Load gate off on Ada's word. Every run's row is in the record.

| rule | both runners voters | write_again max per runner | unknown outcomes | not committed | commits/s | gap before any promotion (best/median/worst) | worst promotion gap (best/median/worst) | passes 200 ms assertion |
|---|---|---|---|---|---|---|---|---|
| exact (today) | 0 of 8 | 128-256 | 18, every run | n/a | n/a | n/a | n/a | 0 of 8 (not reached) |
| G exact-then-slot | 8 of 8 | 4-8 | 0 | 0 | 5.95-6.80 | 366 / 639 / 830 ms | 497 / 806 / 969 ms | 0 of 8 |
| D pages-then-slot | 8 of 8 | none-2 | 0 | 0 | 6.76-8.45 | 172 / 305 / 348 ms | 475 / 584 / 784 ms | 0 of 8 |

kiln's 200 ms gap assertion fails under every rule; gaps past 200 ms appear before any promotion (G 8 of 8, D 6 of 8):
kiln step 10's question.

Found while reading: set 1's exact round 1 box-*.log are a 2026-11-09 run's (the copy loop took an older tempdir);
its row comes from test.log, which carries this run's runner lines. Logs on cec37f60 left in place, nothing removed.
