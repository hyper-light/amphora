# Lane P20 (serve): QB.2's shed keeps the records raft sends once (2026-11-24 UTC, adacachy2)

Goal: every replicated write either commits or gets a typed refusal, and no long write costs the group its leader
(CP-T1.13). This hour's deliverable: perf/serve-p7 on main, done when V13's real-time bench ran on the branch and on
main, the outgoing queue's shed never drops a message raft sends once (a red test on main's code first),
seed_and_delta passes, both tests pass 3 of 3 with one clean round and both cp_t1_13 tests, and the branch is merged
with rustfmt clean.

## Losses first

- perf/serve-p7 is NOT on main. V13's real-time bench (b1, r174) was still building its release binaries at 04:00Z
  (adacachy2 gave 2 cores), so decision 1 could not be applied: V13's clock hold (`perf/serve-p20-v13hold`) is built,
  checked and pushed, not landed. Without the hold V13 fails on the branch (r151, `ae31667f`), so the branch does not
  meet item 4 and was not landed either.
- Landed instead, as rule 5a's "merge what's green": the shed fix with its red test (`perf/serve-p20-shed`), which is
  main's own defect and does not depend on the branch.

- The macOS run (r96, needs macos/arm64,cpus=8,mem=24g) waited queued from 03:03Z to 03:45Z and never started; it was
  stopped once the same steps ran on adacachy2 (r151) after the verifier's window.
- The first real-time bench attempt failed to build: bench/ has no tracked Cargo.lock, so `--locked` refuses. b1 (r174)
  builds it with `--offline`; adacachy2 had 2 to 3 idle cores, so b1 and c1 ran at cpus=2 after cpus=4 was refused.
- Main moved in crates/ four times in the hour (9fbfdbaf, fac69083, c67455f6, 2571f841); each merge cost a rebuild
  and a re-check.
- Not done: the coordinator's V16 fix (perf/commits-v16 dc9b0c81) is not merged here. seed_and_delta passes 3 of 3
  without it once the shed keeps the hand-off order, so no further trace of the late closes was needed this hour.

## What changed

- `cb6756fa` (on main's code, branch `perf/serve-p20-shed`): QB.2's shed takes only records raft sends again. Each
  queued record keeps its raft kind (`Queued`); the shed removes the oldest record of a kind `resent` names
  (`MsgAppend`, `MsgAppendResponse`, `MsgHeartbeat`, `MsgHeartbeatResponse`) and keeps every other. With nothing left
  to shed the new record is queued past the learned bound, as a lone record past it was before; the memory budget's
  room (MB.1) still fences. Red test first: `fa7c515e`, `qb2_the_shed_never_takes_a_hand_offs_order_to_campaign`.
- Kinds raft sends once, so never shed (vendor/hyper-raft `raft.rs`, read this hour):
  - `MsgTimeoutNow`: sent by `handle_transfer_leader` when the heir is at the last index, again only on an append
    answer that brings it there (`handle_append_response`); `propose` refuses every proposal while `lead_transferee`
    is set (`Dropped::Transferring`), so a shed order stalls the group's writes until the hand-off times out, and on
    driven time with the clocks still, for good (P19's seed_and_delta hang).
  - `MsgSnapshot`: `unreachable()` leaves a `Snapshot` progress as it is, and replication to the member pauses until
    `report_snapshot` (hyper-durable `replica.rs`), which amphora never calls. A shed snapshot stalled its member for
    good; now it stays queued, and past the memory room the owner fences, typed.
  - `MsgRequestVote`, `MsgRequestPreVote` and their answers: once a campaign.
  - `MsgReadIndex`, `MsgReadIndexResp`, a forwarded `MsgPropose` or `MsgTransferLeader`: once each.
  - `MsgFastPropose`, `MsgFastVote`: no resend established, so kept.
- `377b15ce`: rustfmt of the branch's long-write test in `tests/serve.rs` (P7's), which `cargo fmt --all --check`
  failed on.
- `8e0eced3`, `493413ab` (branch `perf/serve-p20-v13hold`): V13's moving-time test holds every clock while a session is
  out on a worker, citing replication.md "Decision (lane P6)", 5 ("The worker moves where a session runs, not when"),
  and prints how many of its rounds moved the clocks.

## Results

| check | tree | result |
|---|---|---|
| QB.2 red test | `fa7c515e` (main 7de5344d + test) | FAILED: "the order to campaign was shed: [(2, MsgHeartbeat), (3, MsgHeartbeat)]" (r151) |
| QB.2 test | `cb6756fa` (main + test + fix) | ok (r151) |
| QB.2 test | `143b82cd` (branch + main 9fbfdbaf + fix + hold) | ok (r151) |
| seed_and_delta, 3 runs | `143b82cd` | 3 of 3 ok, 0.74 to 0.88 s (r151) |
| V13, 3 runs | `143b82cd` (hold) | 3 of 3 ok, 6.0 to 6.8 s; clocks moved in 659 of 1036, 701 of 925, 663 of 887 rounds (Persisted) and 569 of 806, 570 of 790, 565 of 777 (Replicated) |
| V13 | `ae31667f` (branch + main + fix, no hold) | FAILED in 1.79 s (r151), as P19 saw on the branch |
| seed_and_delta, V13 | `fa7c515e` (main's product) | both ok (r151) |
| cp_t1_13, lib | `143b82cd` | 16 of 16 ok, both session_worker tests among them (r151) |
| one round: `database::replicated:: database::slot:: replication::` | `143b82cd` | 189 passed, 5 failed (73 s): the floor test (known flaky) and main's four (B19's): checkpoint pass, image-install read, two replica log room |
| one round: the serve binary | `143b82cd` | 11 of 11 ok, cp_t1_13_sql_takes_bound_parameters_and_answers_typed_rows_with_their_names and cp_t1_13_a_replicated_write_past_an_election_timeout_commits_and_keeps_its_leader among them |
| clippy -p amphora --all-targets -D warnings | `143b82cd` | exit 0, 48 s |
| cargo fmt --all --check | each pushed head | clean |

After main moved in crates/ (fac69083, c67455f6, then lane D4's T1 reviewer landing 2571f841), each candidate was
merged again and its touched tests and clippy re-run (c1 r185, c2 r196, c3 r202):

| tree | tests | clippy |
|---|---|---|
| `0de72a59` (shed fix + main fac69083) | QB.2 ok, the drain test ok | exit 0 |
| `201534be` (branch + fix + hold + main fac69083) | QB.2, seed_and_delta, V13 (hold), cp_t1_13 16 of 16, the drain test: all ok | exit 0 |
| `10315a9f` (shed fix + main c67455f6) | QB.2 ok, the drain test ok | exit 0 |
| `0b87e076` (branch + fix + hold + main c67455f6) | QB.2, seed_and_delta, V13 (hold), cp_t1_13 16 of 16, the drain test: all ok | exit 0 |
| `92654cd2` (shed fix + main 2571f841, landed) | QB.2 ok; main renamed the drain test a_whole_queue_drain_survives_a_shrinking_learned_bound: ok (r209) | exit 0 |

## The real scenario (decision 1)

Not measured this hour: b1 (r174) began at 03:52Z on 2 cores and had not finished its release builds when the hour
ended. Its log is on adacachy2 under `~/.cache/amphora-perf/serve-p20-b1`; the binaries go to
`~/.cache/amphora-perf-bin/serve-p20/<sha>/bench`. A successor runs `b1.sh` (or only its cells, if the binaries are
there) and applies decision 1 to `perf/serve-p20-v13hold`.

## Runs

All on adacachy2 (00aad8da...) through kiln `run`, sync false, in the remote clone `~/.cache/amphora-perf/serve-p7-remote`
fetched from GitHub, CARGO_INCREMENTAL=0, the lane's target dir `~/.cache/amphora-perf-target/serve-p7`, scripts in
`bench/records/2026-11-23-adacachy2-perf-serve-p20/`.

| run | needs | script | what |
|---|---|---|---|
| r151 | cpus=4,mem=16g, jobs 4 | l1.sh (2 rounds) | the tests above, clippy, the round; its bench step failed to build (`--locked`) |
| r174 | cpus=2,mem=8g, jobs 2 | b1.sh | release bench of the branch and main, V13's cell alternated; seed_and_delta on the branch without the fix |
| r185 | cpus=2,mem=8g, jobs 2 | c1.sh | the landing checks after main moved in crates/engine |
| r196 | cpus=2,mem=8g, jobs 2 | c2.sh | the same after main moved again |
| r202 | cpus=2,mem=8g, jobs 2 | c3.sh | the shed fix alone after lane D4's landing |
