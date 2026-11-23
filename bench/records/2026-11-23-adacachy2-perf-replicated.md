# Lane replicated-K5, 2026-11-23, adacachy2 (runner 00aad8da, needs linux/amd64,cpus=8,mem=24g)

Correctness work, no timings: the four replicated tests K4 found failing on main
(`2026-11-23-adacachy2-perf-constants.md`), each with its cause, the state that showed it, and its
fix. None is K4's family (a member window of about one record, hyper-raft holding a commit back
until a heartbeat the still clock never reaches): no fix here moves a clock.

## Commands

On adacachy2, scripts in `~/.cache/amphora-perf/k5-scripts/`, a GitHub clone in
`~/.cache/amphora-perf/replicated-remote`, target `~/.cache/amphora-perf-target/constants`
(CARGO_INCREMENTAL=0), logs in `~/.cache/amphora-perf-bin/replicated/logs`:

- `build.sh <rev>`: the lib test binary of `<rev>`, copied to `lib-<sha>`.
- `t.sh <rev> <runs> <parallel> <tag> <test...>`: each test alone in its own process
  (`--exact --test-threads=1`, timeout 900 s), `<parallel>` processes at once.
- `gate.sh <rev>`: build, the focused tests beside clippy, then `database::replicated::` at 8
  threads.

## The four tests (passes of runs, each alone)

| test | main 1e874db1 | fix | cause |
|---|---|---|---|
| compaction_waits_for_the_engines_flush | 0/1 (r588), 0/1 (r605) | 3/3 (r612, 37008250) | the test's compaction woke `Waker::noop()` |
| a_member_installed_from_a_snapshot_holds_the_floors_rows | 0/1, 0/1 | 3/3 | the same |
| a_host_waits_for_a_leader_on_the_owners_word | 0/1, 0/1 | 3/3 | a real-clock deadline to an owner on a driven clock |
| a_follower_past_its_room_applies_on_and_fences_when_its_disk_fills | 1/1, 6/9 | 12/12 | the driver waited on a flush the test held |

Neighbours run with them on the fix, 3/3 each: a_local_read_open_across_an_image_install_sees_one_side_whole
(the third test that compacted with a noop waker), a_held_database_flush_stalls_no_persisted_owner_step
(the other test that holds a flush gate).

r605 ran main plus the hang signal (92fb2d68: the driver's hang names each owner it waited on as
(owner, log writes out, database flush out)) and a print of the lone write's answer.

- **The two compaction tests.** Each compacts its owner's log itself through `Replica::compact`
  with `Waker::noop()`, so the compaction's end rang nothing. The driver on events (35f77526)
  waits after a round that relays nothing for an owner's bell while a write is out. r605: both
  hung with out `[(0, 1, false)]`, owner 0's one write out (the compaction): in `Driven::quiet`
  right after it (compaction_waits), in the first `Driven::round` after it (a_member_installed).
  Fix: the owner's own waker (`Actor::waker`), as the owner compacts (61c8cbee).
- **a_host_waits.** The lone voter's write, meant to fail the full disk's flush, was due at
  `Instant::now() + HANG_READ` on the real clock; its owner's driven clock reads an hour past the
  real one (`Driven::empty`) and refuses a submit past its deadline at intake. r605: the write
  answered `Err(Timeout)`, the owner's clock 3480 s past its deadline; nothing applied, so no flush
  failed and the owner never fenced. Fix: due at `lone.until()`, the fixture's budget on these
  clocks (1097e67b).
- **a_follower_past_its_room.** r605: 3 of 9 runs hung, each in the writes after `gate.hold()`,
  out `[(1, 0, true)]`: the learner's database flush, held at its `FlushGate`, which ends only when
  the test opens it and rings nothing as it waits. Fix: the driver counts a flush only while its
  member's flushes aren't held (`FlushGate::is_held`), as it already skips a write parked at a held
  log gate (b7d508df).

Landed on main as dc14933c (lane head ccdaaa34, clippy `-p amphora --all-targets -D warnings`
clean, r628; ccdaaa34 differs from 37008250 only by an if/else for clippy's disallowed
`bool::then_some`).

## The replicated module's suite (`database::replicated::`, 8 threads, 135 tests, 4 ignored)

r628 ran both arms in turn on the same binaries, r612 the fix once before; load 42 to 83 on 32
cores, other lanes' work beside it:

| run | main 1e874db1 | fix |
|---|---|---|
| r612 | | 130/131 (37008250): t_t1_10_r1 |
| r628 #1 | 127/131 (load 59): a_change_past_its_deadline, a_host_waits, a_member_installed, compaction_waits | 131/131 (load 42) |
| r628 #2 | 127/131 (load 83): a_host_waits, a_local_read_open_across_an_image_install, a_member_installed, compaction_waits | 129/131 (load 55): a_change_past_its_deadline, every_falsifier_of_the_floor_holds_under_a_nemesis |

The fix arm's three suite failures are in tests that no change here reaches: none holds a
`FlushGate` (only a_held_database_flush_stalls_no_persisted_owner_step, a_host_waits and
a_follower_past_its_room take one), so `Driven::movers` names for them what `moving()` did on main,
and the waker and deadline changes are in other tests' bodies. Alone (`--exact
--test-threads=1`; r657 at load 69 to 92, fix only; r673 at load 639 to 935, both arms; r657's
script printed every rc as 0, so its counts are read from each log's result line):

| test | main 1e874db1 alone | fix alone | suite failures, main / fix |
|---|---|---|---|
| a_change_past_its_deadline_answers_what_its_stage_knows | 2/3 | 4/6 | 1 of 2 / 1 of 3 |
| every_falsifier_of_the_floor_holds_under_a_nemesis | 3/3 | 5/6 | 0 of 2 / 1 of 3 |
| session_reads::t_t1_10_r1_a_session_reads_its_write_on_a_lagging_follower | 3/3 (r621) | 6/6 (r621, r657) | 0 of 2 / 1 of 3 |

a_change_past_its_deadline fails on main alone with the fix arm's panic, so it is main's own. The
other two are not yet seen failing on main in 5 runs each; at the fix arm's rates (about 1 in 5)
five clean runs happen by chance about a third of the time, and no path from this lane's change
reaches them. They are open for the next lane:

- a_change_past_its_deadline_answers_what_its_stage_knows: `Pump::admit`'s `admit_receive(..)
  .unwrap()` on `Err(Full { limit: 2 })`, alone on both arms and in the fix arm's suite; "the group
  settled not reached in 60000 driven rounds" in main's suite. The limit is the member's command
  queue bound (`Control::command_bound`, learned as `Bounds::commands`, lane constants-20) at 2:
  the test pump unwraps a refusal of admission at that bound. r683 (main plus a print of each
  member's bounds at the refusal, 12 runs alone, load 200 to 300): 8 passed, 3 `Full`, 1 "settled
  not reached". At each `Full` the leader (member 1) had learned a command bound of 2, the other
  three about 485 million (485001426, 484991969, 484982528), and the refused record was a
  `MsgAppendResponse` from 4 to 1 with the pump's records at 2. The pump checks its room against
  `clients.first()`'s bounds and its own records only, not the destination's inputs already
  queued (a call takes one of a bound of 2's places). The production host drops a record refused
  for room (`serve.rs`, `let _ = handle.receive(..)`); the pump panics. K3's busy-time Drain
  (b64bdc2a, not on main) is not its fix: r691, alone, one at a time, load 100 to 250: main
  1e874db1 9/12, db677a78 (main 9462f146 plus b64bdc2a) 8/12, `Full { limit: 2 }` on both. Open:
  why the leader alone learns 2 (the test moves its clock a second at a time past each change's
  deadline) and whether that bound is right for a member whose clock jumped.
- every_falsifier_of_the_floor_holds_under_a_nemesis: the pump's admission panicked on
  "MsgPropose from 2 (wire 4) to 1 (wire 1), term 0 refused: Invalid { what: \"replica sender\",
  why: \"authenticated sender or destination differs from record\" }"; a lone pass printed a
  seed 12 panic ("learner 4 has not attested kiln.host-store under its current join").
- session_reads::t_t1_10_r1_a_session_reads_its_write_on_a_lagging_follower: the token read was
  served at once (`SessionCounters { at_once: 1, waited: 0, .. }`), so the follower had applied
  2 before the read.

# Lane replicated-K6, 2026-11-23, adacachy2 (runner 00aad8da, needs linux/amd64,cpus=4,mem=16g)

Correctness work, no timings: why the leader alone learns a command bound of 2 in
a_change_past_its_deadline_answers_what_its_stage_knows, and its fix. Scripts in
`~/.cache/amphora-perf/k6-scripts/` on adacachy2 (K5's `build.sh` and `t.sh`, `check.sh`: build,
T-QB.1.17 3x and the deadline test 12x alone, `database::drain::` and `database::patience::`,
clippy `-p amphora --all-targets --locked -D warnings`); logs in
`~/.cache/amphora-perf-bin/replicated/logs`.

## The cause, from the samples

r758 ran main 7b82eeb6 plus a print of each rebound's intake (branch perf/replicated-k6-trace
b6b879ec), the test alone 6 times, load 67 to 77: 5 passed, 1 `Full { limit: 2 }`, every run the
same samples. Only the leader (member 1) rebounds: the test moves only its clock, a second at a
time; the followers' clocks stand still, so they never weigh an interval and hold the room's
envelopes (484966445 and the like). The leader's samples, election timeout 200 ms (20 ticks of
10 ms):

| leader's clock | channel | paused | change held | `queued()` | taken since the last sample | sums after (messages, seconds) | command bound |
|---|---|---|---|---|---|---|---|
| 1 s | 0 | 0 | yes | 272 | 11 (not weighed: first sample) | 0, 0 | 484966445 (room) |
| 2 s | 0 | 0 | yes | 272 | 5 | 5, 1 | 5 x 0.2 = 1, floored: 2 |
| 3 s | 0 | 0 | yes | 272 | 26 | 30.4, 1.875 | 3 |

1. Which intervals it weighs: each whole second the clock jumped, as reading time. The leader took
   those records at one reading (the clock stands still between jumps), then waited a second with
   nothing to read.
2. Dry and came: on main the intake has no dry mark; the tick's sample marks the interval busy
   from `queued() > 0`, the bytes still charged to work the owner took. The leader alone holds the
   change it was asked (272 bytes, `changing=true`) while no input waits (channel 0): it is the
   leader's intake that differs, because only the leader holds a change. K3's busy-time Drain
   (b64bdc2a) adds dry/came but marks dry only in the owner thread's loop, which the driven tests
   bypass (`Driven::step` calls `run_step`), and keeps `queued() > 0` at the tick: on its branch
   the same seconds are weighed (r691: 8/12).
3. A stall at the deadline: not counted at rate zero here (the seconds carry 5 and 26 records), but
   counted at a record or a few a second, which the 1/8 decay brings back up only over tens of
   intervals; the test's next rounds refused before that.
4. The floor `.max(2)` is derived, a call and a wire record (lane constants-20), and not the cause:
   the rate gave 1, the floor raised it to 2. It stays.

The pump: it checked room against member 1's bound and its own receipts only, never what the
destination's channel already held. K5's r683 refusal came with the pump's records at 2, the refused
one counted: at most one other pump record could hold a place in the leader's channel, so at least
one input the pump never sent held the other (measured by that count). That it was the test's own
change call, asked before the round that relays, is inferred, not traced.

## Fix (perf/replicated-k6)

- 66831bc1: perf/constants-k3m (db677a78, K3's busy-time Drain and patience test fix) merged.
- 82751d6a: T-QB.1.17 `t_qb_1_17_a_leader_holding_a_change_admits_what_it_reads_at_once`.
- ed277869: the tick's sample asks whether inputs wait (`Actor::inputs_wait`: the channel's places
  or QB.2's paused inputs), not `queued() > 0`; a step with no input marks the queue dry at its
  reading (`run_step` -> `ran_dry`), as whoever steps the owner found the channel empty. The pump
  relays a record only while every member's command queue has a place by its own bound, less what
  its channel holds; the admission's `unwrap` stays.

## Results (passes of runs, each alone)

| check | before | fix |
|---|---|---|
| a_change_past_its_deadline alone | main 2/3 (r673), 9/12 (r691), 5/6 (r758) | 12/12 (r781, 056449fd), 12/12 (r796, ed277869) |
| T-QB.1.17 alone | 0/3 on 82751d6a (r796): `[Ok(()), Ok(()), Err(Full { limit: 2 })], the leader's command bound 2` | 6/6 (r796, ed277869) |
| `database::drain::` and `database::patience::` | | 19/19 (r781, r796) |
| clippy `-p amphora --all-targets --locked -D warnings` | | clean (r781, r796) |

| on the merge with main, d201b0c2 (r801, load 26 to 67) | | T-QB.1.17 3/3, the deadline test 12/12, drain and patience 19/19, clippy clean |
| `database::replicated::`, 4 threads, d201b0c2 (r801) | main 1e874db1: 127/131 (r628) | 134/134, 4 ignored, 45.8 s |

r781 ran an earlier head (056449fd, the same product change) whose T-QB.1.17 dropped its pump
unfinished (2 of 3 runs: "fixture pump dropped with unsettled records"); ed277869 finishes it as
every driven test does. Load 30 to 77 on 32 cores throughout.

Who notices (inferred from the code, not measured on a real member): a leader holding a write,
a change or a hand-off across ticks no longer counts the time that work waits as reading time, so
its command bound no longer falls toward its least while it reads at once, and its hosts and peers
are not refused `Full` for it. Whether the macOS nemesis's `Full { limit: 2 }` (K3, r391) had this
cause is not checked. What the next owner inherits: the intake's busy flag is
`Actor::inputs_wait`, and every caller of `run_step(None)` marks the queue dry.
