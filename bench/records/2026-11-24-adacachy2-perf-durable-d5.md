# Lane D5 (durable): R28/R30, each lock's publication stamp its durable vote, on main (2026-11-24 UTC)

Goal: Amphora never loses a write, so a recovered row carries exactly the stamp the client heard (T-T1.9.R28, R30; the
coordinator's decision of 2026-11-24 02:03Z: each lock durably keeps its reply's group-clock stamp on every group,
recovery uses the largest, one round, the wire only grows). The lane's hour is 06:57:25Z to 07:57:25Z.

## Losses and limits first

- Nothing here was timed. A lock step on a backed-up group now holds the writer's clock through its COMMIT, and a lock's
  entry on a replicated group can't move past another entry nor be moved past (`readsets`: its vote's pages are written at
  apply, so they are in no read set). A write prepared before a lock's entry is refused `Serialization` and asked again.
  What either costs is unmeasured.
- A member that predates `LOCK_DELTA` finds no valid page list in a lock's entry and refuses it. Amphora is unreleased
  (no compatibility kept), so nothing carries an older member past it.
- What a version-8 coordinator left in flight isn't recovered at 9 (the decision of 02:09Z). A rule for recovery across
  versions is still owed before a release that supports mixed fleets.
- The preflight the decision asks for was in neither main nor the reviewer's branch, so this lane built it (below). It
  covers a participant that was too old at its last greeting. A node replaced by an older one while a transaction runs is
  refused its lock before it goes out; one replaced after its lock is not recovered (the same open release item).
- The power cuts here keep whole writes or none. A torn write inside one is the engine's own checksum tests' business.

## What landed

| Commit | What it does |
|---|---|
| `21215975` | Merges `44b95a5f` alone (its parent `3b8620fa` is on main since lane D4) into main `75efff54`. Two conflicts. `replication.rs`: main prepares a session through `prepare_at`, on the owner's connection or the session worker's (lane P6), so the lock's transaction and the opaque read set moved there. `xgroup/sql.rs`: main's step declares its answer's bytes and rings waiters through `call_internal_ringing`, so `apply_step` answers the reply with the transaction it locked for (`AppliedStep`, charged by `AnswerBytes`) beside the dropped keys. |
| `9dcd7a73` | The edge tests (`xgroup/tests/lock_edges.rs`), tests only. |
| `567f045f` | The preflight: `Nodes` keeps the version each node's last greeting agreed on, and `Cluster::transact` refuses, before any lock, a transaction one of whose participants speaks less than version 9; `Step::Lock` is itself sent only at 9. The wire's and the log's bytes pinned. |
| `21fb0687`, `aca5cef3` | Merge main `04aee7c9` (lane PG18) and `b53a29b1` (lanes B23 and C5). No conflicts. |
| `d167c75c`, `ece9982b` | Two fixes to the new tests that the lints and the compiler asked for. |

## What `44b95a5f` promises (T-T1.9.R28, R30)

Each participant keeps one vote a transaction, in `__amphora_xg_lock_at`, in the WAL commit that makes its locks visible:
a backed-up group's at the clock the writer reserves and holds through that COMMIT, a replicated member's at the entry's
final closed time, lifted where the group's clock was ahead. The lock's reply carries that vote, not the clock read after
it. Parallel Commits still answers after one round. A recovery takes the largest vote, and every row in every group,
those of a replicated group and of one that keeps no clock included, is written at it.

## Red and green on the merged tree

Red (r957, Mac Studio 2) is `ece9982b` with main's own stamping put back in three files and no other change (`put_lock`
stamps with the writer's clock as it runs; a replicated group keeps none; the reply carries the clock read after the step;
an unbacked writer stamps nothing; the simulator's lock keeps the stamp it took). Of the 19 tests named R28 to R31, 10 fail:

| Test | What main answers |
|---|---|
| `t_t1_9_r30_a_recovered_transaction_resolves_at_its_one_stamp` | the client hears 50, every recovered row is at 21 |
| `t_t1_9_r30_a_backed_up_lock_keeps_its_publication_clock` | published at 50, the reopened vote is 21 |
| `t_t1_9_r30_a_replicated_lock_keeps_its_lifted_time_on_every_member` | no vote |
| `t_t1_9_r30_real_owners_recover_the_answer_from_their_locks_alone` | the replicated group has no vote |
| `t_t1_9_r30_a_delayed_lock_reply_keeps_its_own_apply_time` | no vote |
| `t_t1_9_r30_a_crash_at_every_write_keeps_a_lock_and_its_vote_together` (new) | the vote is 21, the lock was published at 50 |
| `t_t1_9_r30_a_member_that_applies_its_log_again_keeps_the_vote_it_answered` (new) | a lock and no vote |
| `t_t1_9_r30_the_order_replies_come_in_changes_no_stamp` (new) | heard 50, recovered 21, in each order |
| `t_t1_9_r30_whichever_groups_missed_the_stamp_recover_at_it` (new) | heard 50, recovered 21, where nothing after the locks landed |
| `t_t1_9_r30_a_group_that_keeps_no_clock_recovers_at_the_others_vote` (new) | the unbacked group's row is stamped 0, the client heard 24 |

The preflight's test is red too with its two lines put back (r957): `TwoPhase: Ok(Aborted(Lost))`, the lock step having
gone out to the version-8 owner. The three tests that pin bytes pass either way, as they should. Green (r956, the same
machine): all of them, in 156 passed, 0 failed.

## Checks on the landing tree `aca5cef3` (main `b53a29b1` merged)

RUNS

## Found on the way, one line each

- `Cluster::transact` computed the groups a transaction touched and dropped them; the preflight uses it.
- kiln's sync to Mac Studio 2 and MacBook Pro 2 timed out (r828, r833, r844, r871), and adacachy2 refused most asks with 0
  or 1 of 32 cores idle. The Macs got the tree as a git bundle through `kiln run --stdin`.
