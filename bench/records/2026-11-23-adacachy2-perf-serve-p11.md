# Lane P11 (serve): the two failing replicated tests, and pages as bytes (2026-11-23, adacachy2)

Goal: every replicated write of any size commits or is refused typed, and no long write costs the group its leader
(CP-T1.13). This hour: why `a_raise_whose_base_is_joint_is_refused` and
`a_slot_holder_whose_write_fails_ends_its_slot_without_waiting_its_length` fail on perf/serve-p7 and pass on main; then
P10's finding on `Prepared.pages`.

All runs through kiln `run`, `on` adacachy2 (00aad8da...), needs `linux/amd64,cpus=4,mem=16g`, CARGO_INCREMENTAL=0,
CARGO_BUILD_JOBS=4, debug test builds in P7's target dir, remote clone ~/.cache/amphora-perf/serve-p7-remote at fixed
SHAs. Logs on adacachy2: ~/.cache/amphora-perf/serve-p11-*.log. Scripts: ~/.cache/amphora-perf/serve-p11-scripts on
adacachy (probe.sh, check.sh, run3.sh).

## Not done (losses first)

- perf/serve-p7 is NOT merged to main. At d682c1aa (r1426) the replicated suite is 144 passed, 2 failed, against main's
  144 passed, 0 failed (r1412, origin/main 21b36021 plus a test-only trace). Failing:
  `a_slot_holder_whose_write_fails_ends_its_slot_without_waiting_its_length` (one assertion later than before, below) and
  `a_raise_beside_a_change_holds_through_a_lost_lead` ("the change was never proposed", replicated.rs:14220; one run).
  `--test serve cp_t1_13`: 3 of 4, `cp_t1_13_a_leader_request_made_during_an_election_is_answered_once_it_is_chosen`
  failed (`RecvError`, tests/serve.rs:383; one run).
- r1446, the branch without the slot change (653a84c4 = d682c1aa with abd8f62f reverted), two suite runs: 143 passed,
  3 failed each. Both runs: `a_refused_follower_write_lands_on_its_next_try_under_the_slot` and the slot test; the
  third differs (`no_writer_is_refused_twice_in_a_row_when_every_member_writes`, then
  `session_reads::t_t1_10_r3_a_session_reads_its_writes_across_leader_changes`). Serve cp_t1_13: 4 of 4. So abd8f62f
  restores what fails without it; the lost-lead and election failures at d682c1aa are one run each and not attributed
  (this area fails a rotating test under load 22-35).
- The A/B of the page encoding (worker encode, apply decode, 20 MiB, alternated) was not run.
- Part 2, part 3's apply half, and small-write latency at 1 and 32 writers: not started.

## Unit 1: the two tests

### a_raise_whose_base_is_joint_is_refused: the test asked for a timing the design no longer gives

Probe: the pump prints every record it carries (`P11TRACE`, probe commits 4eb8c4f3 on main, 0dd5b0fa on the branch,
r1412). Measured:

- Both trees: during the first raise the barrier's heartbeat round runs (3 heartbeats, 3 responses), and the leader
  answers the responses with empty appends at index 9 to members 2 and 3 (`MsgAppend idx 9 n 0`: it believes they hold
  the way-out entry, 9).
- Main: those appends are carried after `appends_lost = false`. Members 2 and 3 reject them, the leader sends entry 9
  (`idx 8 n 1`), both accept, commit 9, the group settles.
- Branch: the same appends are carried before the raise is answered, while appends are still lost. Afterwards, with
  the clock standing still, nothing sends index 9 again: `the group settled not reached in 60000 driven rounds`.

Cause: main answers the raise in the step that runs its session, so the pump has not yet carried what that step sent.
On the branch the session runs on the worker and the owner steps on meanwhile (replication.md, "Decision (lane P6)":
the owner goes on ticking, heartbeating and voting), so the pump carries those records during the session. The product
sends the same records in both. What the test relied on is that nothing the leader sent between the barrier and the
answer is carried before the answer, which the design no longer promises. The design's rule for lost appends is in
`a_change_past_its_deadline_answers_what_its_stage_knows`: "What was lost is made good by the leader's next heartbeat,
which only its ticks send." d682c1aa makes the test do the same (tick the leader until it leaves the joint
configuration, then `settled`). Every assertion is unchanged. r1426: 3 of 3 pass.

### a_slot_holder_whose_write_fails_ends_its_slot_without_waiting_its_length: a behaviour the branch lost, partly fixed

Read in code, confirmed by the run: `prepare()` sends the holder's second barrier (W3) only when the work ended inside
`prepare_barrier` with nothing pending. On the branch a caller's session goes to the worker (`Phase::Running`), so
`prepare()` took `slotted` and sent nothing, and `decide()` never asked again when the session came back refused. r1412:
the holder's write was never answered (`slot_holds true`, `slot_left 200ms`).

abd8f62f: `prepare()` leaves work out on the worker to `decide()`, and both end the slot through one rule (`end_slot`):
a forward ends it at the leader, anything else sends the second barrier. r1426 (3 of 3 runs): the holder's write is now
answered and the leader's write applies, but member 2's write is refused `Serialization { "the database advanced since
preparation" }` (replicated.rs:19343), where main applies it. Not explained this hour. Next step: the same pump trace
on both trees from the holder's answer to member 2's answer, plus the leader's `slots` state and member 2's `tail`,
to see whether member 2 prepares before the leader's own write applies on the branch and after it on main. Per Ada's
direction (11-23, after this hour's runs): run the scenario with a running clock and a real network, or on the ladder;
if the product commits both writes there, fix the test's setup to match and cite the design; fix the product only if
the real scenario misbehaves.

## Unit 2: a write's page images as byte strings

P10's finding, verified in code: `Prepared.pages: Vec<(u32, Vec<u8>)>` goes through serde as a sequence of `u8`, one
call a byte, in `Prepared::encode` (the worker) and `Prepared::decode` (every member's apply). `page_images` writes
each image with `serialize_bytes` and reads it with `deserialize_byte_buf`. postcard writes a byte string as its varint
length then its bytes, which is what it wrote for the `u8` sequence (postcard writes a `u8` as one raw byte), so every
entry's bytes are unchanged and no version moves. `pages_encode_as_before_and_decode_across` holds it both ways: images
whose lengths take one, two and three varint bytes, empty and whole, encoded identically and decoded across, and a
whole entry with a baseline through `Prepared::decode`. No new dependency (`serde_bytes` is not in the lock and builds
run offline). Held off main and off perf/serve-p7 by rule 0 (speed work waits for the deliverable): branch
perf/serve-p11-pages, a52a337a on origin/main 21b36021. r1446 there: clippy exit 0; `--lib replication::` 24 passed,
the new test among them; `--lib database::replicated` 144 passed, 0 failed (main's set).

Not measured: the encode and decode times. The A/B (both arms from fixed SHAs, one 20 MiB write a process, alternated,
release builds) is the next lane's first item.
