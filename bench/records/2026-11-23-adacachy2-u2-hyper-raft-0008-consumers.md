# hyper-raft patch 0008 against every consumer's suite (lane U2, 2026-11-23)

## U2

Lane U1 merged Amphora's patch 0008 into hyper-raft's `main` (fix `7d1c7be`, merge `5b48547`). It
changes hyper-durable's `Replica::open` alone: a log compacted before the open has no snapshot made at
open, and the first `Replica::drive` after the core asks for one makes it. hyper-raft's CLAUDE.md §3
lets a change stay on `main` only once every consumer's suite passes against it. U1 ran hyper-raft's
and hyper-durable's suites. U2 ran the consumers'.

Method. Each consumer's suite ran twice on adacachy2 (kiln `run`, `on` 00aad8da, needs
`linux/amd64,cpus=8,mem=24g`, `CARGO_BUILD_JOBS=8`, `--test-threads=8`), from a scratch clone of the
consumer's `main` under `~/.cache/hyper-raft-dependents/<repo>`: as it stands (old), then with
`git diff d5d5f8f 7d1c7be -- crates/hyper-durable/src` applied to its vendored hyper-durable (new). The
two runs differ by 0008 alone. Re-vendoring a consumer to `5b48547` instead would also bring every
change since its own pin, so a failure could not be laid on 0008. The script is
`~/.cache/u2-scripts/u2-ab.sh` on adacachy, and the patch is `~/.cache/u2-0008.patch`.

| consumer | pin | suite | old | new | runs |
|---|---|---|---|---|---|
| mantle `87ab09e1` | `vendor/`, hyper-raft `5e40eb9` (the three files byte for byte `d5d5f8f`'s) | `cargo test --workspace --locked --no-fail-fast` | still running at 19:05Z. Its first binaries fail `a_silently_skipped_cli_upload_is_not_acknowledged`, `the_aws_cli_and_boto3_pass_against_a_cell_of_three` and `a_cell_filled_with_large_objects_refuses_the_next_through_both_doors_and_keeps_none_of_it` | queued after old in `r1407`. In `r1386` (stops at the first failing binary) the same test failed on both | `r1386`, `r1407` |
| kiln `fd5bd770` (Quiver's copy too) | `vendor/hyper-raft` at `e0613f5` (3-way apply, clean) | `-p kiln-cd -p kiln-obs` | pass (372, 229, 4) | pass (372, 229, 4) | `r1394` |
| kiln | same | `-p kiln --lib` (`RUSTUP_TOOLCHAIN=stable`: kiln needs a newer rustc than 1.98.1) | building at 19:05Z | queued after old | `r1408` |
| focal `dd5f80a3` | `vendor/` at `5007f44` (one hunk ported: focal's `compactable_after` kept) | `-p focal-consensus`, hyper-durable's one user (the full suite takes about 80 minutes) | pass (162 + 3) | pass (162 + 3) | `r1397` |
| slates | `vendor/hyper-raft` at `aa82ada` | none: no hyper-durable vendored, so its sources are the same on both | - | - | - |
| Amphora | `vendor/hyper-raft` at `7d1c7be`, patch 0008 dropped | U1's runs | - | - | U1 |

No suite failed on the new run after passing on the old one. Nothing was reverted. Patch 0008 stays
dropped, and Amphora's pin stays at `7d1c7be`.

The missing test. 0008 had no test of its own. hyper-raft `d955df3` (on `main` as `4ee7fac`) adds
`a_member_that_opens_on_a_compacted_log_images_only_once_a_snapshot_is_asked_for`, with the test
`Kv` counting the images it makes. A member reopened on a compacted log makes no image at open, and
makes one when its core asks for a snapshot. The learner behind the log's start is sent that snapshot.
It passes on `main` and fails on `045cf3f`'s sources (`left: 1, right: 0`, "opening makes no image").
hyper-durable's whole suite, clippy (`--all-targets --all-features -D warnings`) and `cargo fmt --all
--check` pass on it (`r1405`).

Open. mantle's full suite (`r1407`) and kiln's `--lib` (`r1408`) were still running when this lane
ended. Each prints `=== U2 ARM old|new exit <rc>` and the failing tests. The logs are
`~/.cache/hyper-raft-dependents/u2-<repo>-<arm>.log` on adacachy2. A failure in new that is absent
from old is 0008's, and under §3 it means reverting `5b48547` on hyper-raft `main` and restoring patch
0008 here. focal's 80-minute suite has not run on 0008.
