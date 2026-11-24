# Lane P7 (serve): a caller's session on a worker of its own, built (2026-11-23, adacachy2)

Goal: a replicated server that answers writes of any size and length; every write commits or is refused typed,
and one long write never costs the group its leader (CP-T1.13). Lane P6 designed the fix
(docs/design/replication.md, "Decision (lane P6)"); this lane built it on perf/serve-p7.

## What was built

- `Sessions` (crates/amphora/src/database/replicated.rs): one thread per owner, its connection opened as the owner
  starts (`Machine::session_connection`, the owner's page cache). A caller's session (`Author::Application`) is
  handed to it at its preparation with its base (`Machine::session_base`: the applied point, the baseline, whether
  the closed time's table exists) and runs `prepare_at`, the code `prepare_db` ran, unchanged. The pending write
  waits in `Phase::Running`. The result comes back on a channel and rings the owner's bell, as `Flushing::Thread`
  does; the owner takes it at its next step with `try_recv`, lifts the hold, drives, then decides: proposed if the
  lead and term are the write's, else `NotLeader` (nothing proposed, a known outcome). The session closure is
  never run again.
- `Replica::hold_apply` (vendored hyper-durable, recorded as hyper-raft `0009`): while held, `apply_range` keeps
  what is committed behind the fence and `release_fence` waits; once lifted, the next drive walks the range as one
  past a drive's page, the commit fence's rule kept.
- While a session is out the owner takes no write lock: `refresh`, the floor's keeper, a promotion's floor check
  (refused `Busy` at once, as a tail still to apply is), the log's restart and the read hold for it all wait.

## What was run

Through kiln `run`, `on` adacachy2 (00aad8da...), needs `linux/amd64,cpus=8,mem=24g`, CARGO_INCREMENTAL=0, debug
test build, script ~/.cache/amphora-perf/serve-p7-scripts/run.sh.

| Test | f1ac17c7 (before) | after |
|---|---|---|
| `cp_t1_13_a_replicated_write_past_an_election_timeout_commits_and_keeps_its_leader` (P5's, 16 MiB) | FAILED, UnknownOutcome (r1004) | ok (r1060, ac731933) |
| `cp_t1_13_sql_takes_bound_parameters_and_answers_typed_rows_with_their_names` (main's, 20 MiB) | FAILED, UnknownOutcome (r1004) | FAILED, UnknownOutcome (r1060, r1101) |
| `session_worker::cp_t1_13_a_leader_keeps_its_lead_and_answers_local_reads_while_a_session_runs` (new) | - | ok (r1101, eb0d3741) |
| `session_worker::cp_t1_13_a_write_whose_lead_moved_while_its_session_ran_is_refused_not_leader` (new) | - | ok (r1101, eb0d3741) |

P5's probe at 20 MiB on eb0d3741 (r1101, load average 43-54): before the write every member said leader 1, term 1,
applied 3. The write answered UnknownOutcome after 2.61 s. After it every member said leader 0, term 2, applied 3
and durable 3: the session ran on the worker and the write was proposed, then the group elected again before the
20 MiB entry was durable on any member. So at 20 MiB the leader is now lost after the proposal, not during the
session. That second stall is not in P6's design; its cause is not yet traced (candidates: the leader's owner
encoding and sending a 20 MiB entry to two followers, a follower's inbound admission bound refusing the record and
the heartbeats behind it, the log write of the entry under I/O pressure).

## The replicated suite and tests/serve.rs on eb0d3741 (r1103, load average 80-96)

`run.sh eb0d3741 test --lib database::replicated`: 130 passed, 8 failed, 4 ignored (main: 134 of 134). Not
mergeable. Each failure and what it shows:

| Test | Failure | What it shows |
|---|---|---|
| `a_write_at_a_follower_is_answered_on_its_groups_commit` | one of two follower writes refused `Serialization` ("the database advanced since preparation") | a follower's session now spans steps; an entry committed meanwhile makes its base stale and its forward is refused at apply, where before it prepared on the tail and proposed in one step |
| `a_refused_follower_write_lands_on_its_next_try_under_the_slot` | refused twice in a row (keys 2, seed 2: [3, 2]) | the write slot's turn no longer covers the holder's preparation, which now takes steps of its own |
| `no_writer_is_refused_twice_in_a_row_when_every_member_writes` | refused twice in a row (keys 2, seed 3: [2, 2, 1]) | the same |
| `seed_and_delta_hold_through_an_election_at_each_step` | `Err(NotLeader { leader: Some(1) })` | a lead lost while a session runs is refused `NotLeader` as the design says; the test expects the write requeued and retried as a write lost before its preparation is. A `Mutation` is data, not a spent closure, so the worker could hand it back for a requeue; only an `Operation::Call` is spent |
| `a_raise_beside_a_change_holds_through_a_lost_lead` | "the change was never proposed" | a promotion's floor check while a session is out is refused `Busy` (`admits_voter`); it should wait for the session, as a raise waits in `Stage::Raising` |
| `a_raise_another_lead_wrote_over_is_answered_unknown_outcome` | `Option::unwrap()` on `None` at replicated.rs:14218 | not traced |
| `a_raise_whose_base_is_joint_is_refused` | "the group settled not reached in 60000 driven rounds" | not traced |
| `receive_receipts::admission_is_bounded_while_processed_receive_waits_and_errors_remain_observable` | the test's session asserted the admitted bytes (33554704 against 33572176) on the worker thread, which panicked; the driver then hung 120 s | the test reads admission from inside its session, which ran on the owner's step before; and a session that unwound left the owner waiting on a bell that never rang (fixed in 4ff28e52: the worker rings as it ends and the owner fences, unverified) |

`run.sh eb0d3741 test --lib serve::`: 11 of 11 ok. `run.sh eb0d3741 testargs --test serve -- --skip p5_probe`:
9 passed, 2 failed: both `cp_t1_13` tests, P5's 16 MiB one included, which passed at r1060 (load average 70).
That the 16 MiB write was lost after its proposal too, as the 20 MiB one was, is inferred, not shown: no probe ran beside it.

`run.sh eb0d3741 clippy` (r1101): exit 0.

## Not measured

Commit latency at 1 and 32 writers on the replicated rung, before and after (RULES 5e calibration copied): not run
in this lane's hour. The successor measures both arms from fixed SHAs (before: a53c457c; after: this branch's
merge) with one shared XDG_CACHE_HOME.
