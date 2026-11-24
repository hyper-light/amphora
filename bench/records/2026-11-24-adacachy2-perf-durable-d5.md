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

## Checks on the merged tree

Main moved inside `crates/` three times in the hour, so each merge was checked. The last, `9529b50c` (main `6c54b2cb`), has
the production code of `aa99c841`: lane A25 changed only a test and its `#[cfg(test)]` hook between them. All macOS runs
are on Mac Studio 2 (32 cores, load 3 to 32, the named cores), all Linux ones on adacachy2 (load 44 to 78, jobs 1 or 2).

| Tree | Check | Result |
|---|---|---|
| `aa99c841` | Linux `cargo clippy --workspace --all-targets -D warnings` (r1037) | exit 0; engine, amphora and the pg crates checked |
| `aa99c841` | Linux `scripts/check-production.sh` (r1037) | exit 0: "10617 left in the engine's production code across 17 lints, as the ratchet holds" |
| `9529b50c` | macOS workspace clippy (r1046) | exit 0 |
| `9529b50c` | amphora lib `xgroup::`, `database::across::`, `transport::`, `replication::`, `session_reads` (r1046, 8 threads) | 123 passed, 0 failed, 2 ignored |
| `aa99c841` | amphora lib `xgroup::`, `database::across::`, `transport::`, `replication::`, `backup::`, `db::` (r1035) | 168 passed, 0 failed, 2 ignored |
| `aa99c841` | `--test serve`, `transport` (r1035) | 11 and 9 passed |
| `aa99c841` | amphora lib `database::replicated::` (r1034) | 161 passed, 3 failed, 5 ignored; below |
| `aca5cef3` | the same lib sets and `database::replicated::` in one run (r984) | 329 passed, 2 failed, 7 ignored; below |
| `aca5cef3` | `--test serve`, `transport`, `groups`, `replication` (r984) | 11, 9, 12 passed; 2 passed and 1 failed; below |
| `aca5cef3` | Linux clippy and check-production (r985) | exit 0, exit 0 |
| `04aee7c9`, main alone | the lib sets with `database::replicated::` (r964) | 305 passed, 1 failed (`v13_...`), 7 ignored |
| each | `cargo fmt --all --check`, `python3 scripts/test_requirements.py` (the duplicate-ID test; already on main) | pass |

Reds, each main's own unless it says otherwise:

- `v13_thirty_two_spread_writers_on_moving_time_...`: "Persisted: row 0 at member 1: Timeout" on every macOS run of the suite,
  main alone included (r964, the same line and message).
- `a_checkpoint_pass_copies_no_frame_past_...` (r975, `ece9982b`): "left: 77, right: 17", main's own in lanes B20's, B21's and
  V20's records. Lane V20 has since changed the test by its design, and it passes on `aa99c841`.
- `local_readers_are_bounded_by_the_transport_not_four`: `Err(Timeout)` at replicated.rs:17174, failed at `aca5cef3`; main `b53a29b1`
  alone fails it 1 of 3, the same line (r1028). Lane B23 landed before both.
- `--test replication`, `stale_preparations_are_refused_and_snapshots_...`: "snapshot must read the current catalog page", on main
  `04aee7c9` alone too (r1029).
- `serve` failed two at `ece9982b` under load 31 (`os error 35`, a read's deadline) and passes 11 of 11 since, as on main.
- `a_group_upgrading_to_the_floor_tracks_joins_...` failed once in r1034 ("owner 1 differs at applied index 3") and passed 3 of 3 alone
  on `aa99c841` (r1052) and 3 of 3 alone on main `1ceb12fd` (r1051). No lock entry is in it. Whether main's suite shows it is in the
  post-merge runs below, if they ended in the hour.
- `a_local_read_open_across_an_image_install_...` failed in r1034 and 3 of 3 alone on `aa99c841` (r1052): "the state machine failed".
  It passed at `aca5cef3`, before lane V20's `32ba72e3` came in. Main `1ceb12fd` alone fails it 3 of 3, the same line and message
  (r1051): main's own since that landing, on macOS. It passed on Linux in V20's runs. A replica's state machine failed, so its owner
  should see it.

## Found on the way, one line each

- `Cluster::transact` computed the groups a transaction touched and dropped them; the preflight uses it.
- kiln's sync to Mac Studio 2 and MacBook Pro 2 timed out (r828, r833, r844, r871), and adacachy2 refused most asks with 0
  or 1 of 32 cores idle. The Macs got the tree as a git bundle through `kiln run --stdin`.
