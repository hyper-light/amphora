# Lane constants-K4, 2026-11-23, adacachy2 (runner 00aad8da, needs linux/amd64,cpus=8,mem=24g)

Correctness work, no timings: three fixes and a rerun of the six replicated tests r389 failed.

## The six tests alone (each in its own process, `--exact --test-threads=1`)

Command: `~/.cache/amphora-perf/k4-scripts/par2.sh <fix>` on adacachy2 (lib test binaries of main
364266aa, b64bdc2a and the fix head, built in `~/.cache/amphora-perf-target/constants`,
CARGO_INCREMENTAL=0; logs in `~/.cache/amphora-perf-bin/constants/par-logs`).

Runs r508 and r547, 12:3xZ to 12:5xZ, load 19 to 37 on 32 cores, at most 3 test processes at once.
Passes of runs:

| test | main 364266aa | b64bdc2a (K3 busy-time Drain) | failure, both arms |
|---|---|---|---|
| t_qb_1_9_a_one_voter_group_grows_to_five_then_nine_members | 0/2 | 0/3 | `Option::unwrap()` on `None`, Pump::admit (no owner for learners 3 to 9) |
| a_follower_retry_racing_the_slots_start_lands_on_its_next_try | 0/2 | 0/3 | "a driven operation was unanswered after 60000 rounds": holder applied 3, leader committed 4 |
| a_follower_past_its_room_applies_on_and_fences_when_its_disk_fills | 1/2 | 1/2 | one 120 s "work was out off the driver's thread ... a hang" each |
| a_host_waits_for_a_leader_on_the_owners_word | 0/2 | 0/2 | `Err(Timeout)` at 60 s (replicated.rs:12635 on main) |
| a_member_installed_from_a_snapshot_holds_the_floors_rows | 0/1 | 0/1 | 120 s hang (driver's thread) |
| compaction_waits_for_the_engines_flush | 0/1 | 0/1 | 120 s hang (driver's thread) |

None is a load flake at this load: the four that fail every time fail the same way alone, and
a_follower_past_its_room hangs one run in two on both arms. b64bdc2a fails nothing main passes.
The fix head (8350cc3a, main plus this lane's three commits) passes t_qb_1_9 3/3 and the
retry-racing test 3/3 (r547, r564); the other four are untouched here.

## Fixes

- t_qb_1_9_a_one_voter_group_grows_to_five_then_nine_members: the pump panicked at
  `Pump::admit`'s `clients.get(to - 1).unwrap()` (`Option::unwrap()` on `None`): the leader sends
  to learners 3 to 9, which the test runs no owner for. They are declared lost, as in every test
  with a learner that does not run.
- a_follower_retry_racing_the_slots_start_lands_on_its_next_try: "a driven operation was
  unanswered after 60000 rounds", the holder at applied 3, the leader committed at 4. W's lost
  append fills the holder's learned window (`Window::Carried`), so `Progress::is_paused` holds
  back the commit's broadcast too; a heartbeat's answer (`handle_heartbeat_response`, a beat of
  the leader's ticks) is what sends it again. The test moves the leader's clock `heartbeat_tick`
  ticks once the loss ends.
- Drain's sums in 64.64 fixed point (K3's finding). Model of the integer arithmetic
  (`~/.cache/amphora-perf-k4/fixed.py`), 40 intervals of 10 ms, 100 ms timeout, true bound 10c
  messages:

  | messages/interval | whole units (old) | 64.64 |
  |---|---|---|
  | 1 | 1 | 9 |
  | 2 | 11 | 19 |
  | 10 | 91 | 99 |

  64.64 lands one under an exact integer (10 less 2^-64, floored); the test uses a 105 ms timeout
  (10.5 messages, 1039.5 bytes) and expects (1039, 10) at interval 1 and 400; the old arithmetic
  gives (1030, 1) at 400.
