# Lane P10 (serve): the proposal's copies off the owner thread (2026-11-23, adacachy2)

Goal: every replicated write of any size commits or is refused typed, and no long write costs the group its leader
(CP-T1.13). This hour: part 3's proposal half of "Decision (lane P9)" (docs/design/replication.md), P8's open item on
the overwritten raise, and r1267's logs read in. Not merged to main: the gate (the replicated suite matches main's,
both cp_t1_13 tests pass) is not met (below).

All runs through kiln `run`, `on` adacachy2 (00aad8da...), needs `linux/amd64,cpus=4,mem=16g`, CARGO_INCREMENTAL=0,
CARGO_BUILD_JOBS=4, debug test builds in P7's warm target dir, remote clone ~/.cache/amphora-perf/serve-p7-remote at
fixed SHAs. Logs on adacachy2: ~/.cache/amphora-perf/serve-p10-*.log.

## What changed (perf/serve-p7)

- 8aaa3a98: a caller's write is encoded (`Prepared::encode`, postcard over every page) and copied into its stamped
  entry (`closed::stamp`) on the session worker, before the bell rings; the owner's step writes its clock into the
  entry's eight stamp bytes in place (`closed::restamp`) and proposes. Owner-run preparations (compaction steps, the
  owner's own sessions) still encode on the owner. What stays on the owner and grows with the entry: hyper-raft's
  copies of the entry into each peer's append and each record's encoding (memcpy), and `rebase` when the leader's log
  holds entries past the write's base (it decodes the write).
- 2c3eb81a: origin/main (0d87fcdd) merged. One conflict, vendor/hyper-raft/UPSTREAM.md: main revised 0009 and removed
  W5's 0011; resolved to main's 0009 row and this branch's 0010 (the apply hold).
- 54ff73c9: (a) a proposal made after the step's drive steps the owner again at once. `Actor::decide` proposes a
  worker's write after the drive and `release`, and `run_loop`'s `driving` was set before it, so the owner slept to its
  next tick or input before sending the append and writing it: up to one pace period on every worker write (read in
  code, not measured). (b) `a_raise_another_lead_wrote_over_is_answered_unknown_outcome`: the leader steps, nothing
  relayed, until an append was queued and lost (P9's inferred mechanism: the test drained before the append existed).

## r1267, P9's last run, read in

| arm | round 1 | round 2 |
|---|---|---|
| f2562197 = merge + probe + part 1 | void (built from r1257's checkout) | 2 Ok, 5 UnknownOutcome, then "never healthy" (tests/serve.rs:88) |
| 4cc98921 = the same, part 1 reverted | 1 of 10 Ok (r1257 still checking out; load 32) | 8 of 10 Ok |

On main's 0009 part 1 did not do better than its pair in the one round both ran: 2 of 7 against 8 of 10. P9's reading
("part 1 helps") does not hold on main's 0009; its effect is unresolved. The merged head d50695f9: clippy exit 0;
`--test serve cp_t1_13` 4 of 4; `--lib serve::` 11 of 11; `--lib database::replicated` 138 passed, 5 failed
(`a_raise_another_lead_wrote_over_is_answered_unknown_outcome`, `a_raise_whose_base_is_joint_is_refused`,
`a_refused_follower_write_lands_on_its_next_try_under_the_slot`,
`a_slot_holder_whose_write_fails_ends_its_slot_without_waiting_its_length`,
`no_writer_is_refused_twice_in_a_row_when_every_member_writes`).

## Main's replicated suite, the merge gate's bar

r1296, origin/main 18e3388a: `--lib database::replicated` 141 passed, 0 failed, 5 ignored (load 15-24).

## Checks at 54ff73c9 (r1352)

Load average 26-28 on 32 cores.

- clippy `-p amphora --all-targets --locked -D warnings`: exit 0.
- `--test serve cp_t1_13`: 4 of 4, both cp_t1_13 tests among them.
- `--lib serve::`: 11 of 11.
- `--lib database::replicated`: 143 passed, 2 failed, 5 ignored. Still failing: `a_raise_whose_base_is_joint_is_refused`
  (the group never settles, replicated.rs:11361, as before) and
  `a_slot_holder_whose_write_fails_ends_its_slot_without_waiting_its_length` (replicated.rs:7146). Passing now:
  `a_raise_another_lead_wrote_over_is_answered_unknown_outcome` (54ff73c9's test change),
  `a_refused_follower_write_lands_on_its_next_try_under_the_slot` and
  `no_writer_is_refused_twice_in_a_row_when_every_member_writes` (one run, cause not isolated: main's merge or
  54ff73c9's `driving`). Main's bar is 0 failed, so the gate is not met.
- `python3 scripts/vendor.py check hyper-raft`: 1377 files at d5d5f8f6, 9 recorded changes, 0 differences.

## A/B: the 20 MiB commit rate, arms alternated write by write (r1352)

Arm A 99176824 = 2c3eb81a with 8aaa3a98 reverted, plus P8's probe (f2562197's commit); arm B a2a7cb37 = 54ff73c9 plus
the probe. Each arm's `serve` test binary built once and copied to ~/.cache/amphora-perf-bin/serve/<sha>/; then ten
rounds, A then B in odd rounds and B then A in even ones, each write a fresh three-member group (`P5_SIZES=20`). Load
average and CPU pressure (some avg10) read after each write.

Load average 15.7-26.1 and CPU pressure 1.8-2.7% across the twenty writes.

| arm | writes Ok | the leader's proposal step (`P8PROBE`, last index 3 -> 4), ten writes | median |
|---|---|---|---|
| A 99176824 (before) | 10 of 10 | 294.4, 272.6, 279.1, 282.1, 304.6, 301.0, 286.9, 269.7, 273.5, 269.3 ms | 280.6 ms |
| B a2a7cb37 (P10) | 10 of 10 | 7.4, 8.1, 4.7, 3.1, 5.9, 9.8, 3.0, 6.5, 3.2, 4.0 ms | 5.3 ms |

The leader's proposal step falls from 269-305 ms to 3.0-9.8 ms (measured, debug build): the ranges do not overlap, 53x at
the median. Arm A's step matches P9's 310.5 ms and P8's 264.6 ms, so the encode and the stamp's copy were that step's
cost. The commit rate did not separate the arms: at this load all twenty writes committed, so the leader loss P9 saw
did not reproduce in either arm (the followers' election timeout is randomized over 200-400 ms, and a 270-305 ms step
outlasts only part of it; inferred). What is left, the same in both arms (write 1 of each, measured): the leader's
next steps after the proposal at 20-33 ms (inferred: hyper-raft's copies of the entry into each peer's append and their
encoding), the followers' steps on the entry at 13-31 ms, and the apply, unchanged. Small-write latency was not run.

## Not done in this hour

- Part 2 (entries in parts sized by a path's carriage in a heartbeat interval) and part 3's apply half: not built.
- Small-write commit latency at 1 and 32 writers: not run.
- P8's open items: the write slot's turn covering the holder's whole session, `a_raise_whose_base_is_joint_is_refused`
  (the state P9 names not printed yet), the worker's reader taken on the worker thread: not started.
- Seen, not changed: `Prepared.pages` is `Vec<(u32, Vec<u8>)>`, which serde hands postcard as a sequence, one call a
  byte, on encode and decode. postcard writes a byte sequence and a byte string identically (a varint length, then the
  bytes), so encoding the pages as bytes would keep every entry's encoding and cut the per-byte work on the worker and
  in every member's apply (P8's 915-962 ms apply decodes it). Inferred from postcard's format, not measured.
