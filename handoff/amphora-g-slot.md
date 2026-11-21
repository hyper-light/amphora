# amphora-g-slot handoff (goal 9c63f7d4's fix, G: the write slot; 2026-11-11)

Agent amphora-g-slot. Worktree ~/Projects/amphora-t1-g-slot, branch t1/g-slot from the batch head b151af0.
Built for merge. Not merged, not pushed.

## Head
- c6ad588, tree 45ef9a9b25e7ee7c10fd796cc2c3ec74b16192b6 (W1's control made deterministic after the Linux gate's failure at b35e010; on 48347cd: the events' doc line, events/v1, on 0030f95; held receipts answered Ok and a lost lead's holds discarded, amphora-owner-t1-g-slot-fc28e2d-20261112; on 7e1e024's names). Commits: 3cad797, the failing tests (red on today's rule,
  recorded there); 878285a, the slot; fc28e2d, the leader's turn after a slot, held receipts answered as they step,
  and Driven::answer reaching a slot's end in a quiet group.
- Code: crates/amphora/src/database/slot.rs (new pure core); database/replicated.rs (`step_forward`, `step_ready`,
  `keep_slots`, `start_slot`, `slot_ended`, `slot_bounds`, the hold in `step`, the deferral in `prepare`, `tail` for
  followers' writes, the tests); meter.rs (`SlotEnd`, three metrics); database.rs (the module).
- Docs: docs/design/replication.md, "A follower's write under steady leader writes: the write slot";
  docs/requirements.toml AC-T1.8.W1 and T-T1.8.W1 to W4; and, from t1/write-starve 044c44f, the research note and the
  A/B records the decision cites (docs/research/follower-write-starvation.md, bench/records/2026-11-0{7,9}-write-starve-*).
- Seam owner: the design was agreed under amphora-owner-t1-g-slot-design-20261111, and the code at 878285a under
  amphora-owner-t1-g-slot-878285a-20261111. That ruling agreed the barrier-driven start, rejected the leader keeping
  today's rule in its turn (fixed in fc28e2d as it prescribed), and sent the head to the batch. The follow-up notice
  g-slot-turn-20261112 reports fc28e2d, including two fixes the ruling didn't cover (held receipts at a lost lead,
  and the driven harness's slot end). Its answer goes in codex-notices/t1-g-slot.reply.md.
- Kiln Metrics: agreed under amphora-g-slot/names/v2 (label `outcome`, gauge `_length_seconds`, event `replication.write_slot`).

## Tested (adas-mac-studio.local-2, debug, 2026-11-11, load 4 to 7)
All passed:
- cargo fmt --all --check
- cargo clippy -p amphora --all-targets --locked -D warnings (macOS, natively)
- bash scripts/check-production.sh
- cargo test -p amphora --lib (185 passed, 2 ignored), and the floor nemesis 16 of 16 (it hung 2 in 23 before
  fc28e2d's harness fix; base b151af0: 0 in 8)
- cargo test -p amphora --test replication --test application --test kiln_seam --test transport
New tests: W1 `a_refused_follower_write_lands_on_its_next_try_under_the_slot`, W2
`two_followers_and_a_leader_writing_steadily_all_commit_within_their_deadlines`, W3
`a_follower_retry_racing_the_slots_start_lands_on_its_next_try`, and W4, ten tests in database::slot. Numbers:
- W1: the follower was refused at most once in a row (2 to 5 refusals per 15 writes), against 12 in a row on today's
  rule.
- W2: each follower was refused at most once in a row, against 36 and 37 on today's rule. No write the leader made
  in its turn after a slot was refused: turns (12-14, 0) over 3 repeats; (29, 29) without the turn rule.
- Every list-append history was serializable.
Not run (the batch's): the seven gates (Linux and Windows clippy, the workspace test, vendor.py check), e2e,
benchmarks.

## The release A/B, for the batch (CLAUDE.md §7; the command from amphora-write-starve's handoff)
G is now always on, with no env switch. To A/B it, build kiln's promotion test against a vendored amphora carrying
t1/g-slot 3cad797..878285a (control: the same kiln at b151af0's amphora), in release, N=5 interleaved, on a quiet host
under the heavy lock:

```
export PATH=$HOME/.cargo/bin:$PATH CARGO_TARGET_DIR=<a target of its own> CARGO_TERM_COLOR=never RUST_BACKTRACE=1 \
  CARGO_INCREMENTAL=0 TMPDIR=<a dir of its own>
nice -n 10 cargo test --release --locked -j 8 -p kiln --test p2p_group -- --test-threads=1 --exact \
  p2p_a_promotion_to_voter_costs_the_groups_writes_no_gap_past_an_election_timeout --nocapture
```

Record each runner's `write_again` maximum, unknowns, promotions and the gap lines, and the writer's commits per
second. Add the leader's `amphora_replica_write_slot_holds_total{outcome}`, `_held_seconds_total` and
`_write_slot_length_seconds` from its meter (`ReplicaHandle::counters().write_slot_*`). Expected, and falsified if not:
`write_again` at most members − 1 = 2 per write, and the `expired` share within 1/(n + 1) (the decision's falsifiers).
The kiln-leg script on local-2 (~/.cache/write-starve/kiln-leg.sh) passes AMPHORA_ADMISSION_PROTOTYPE, which this
branch doesn't read. Run its cargo line without that variable.

## Open
- The seam owner's answer to g-slot-turn-20261112 (fc28e2d's two fixes beyond its ruling).
- A finding for the Amphora agent: a general tail rule at the leader. Outside a turn, the leader's writes keep
  today's rule. In the driven runs a general rule cut the leader's refusals from up to 8 in a row to at most 1, but it
  moves two floor tests' premises; it would be its own decision.
- TICK and the twenty-tick election timeout, which the slot takes as its first value, are for amphora-constants.

## Asked of whoever merges the batch (2026-11-12)
- Direct Routing (kiln agent 01a1d0a926e07b71a209e15eb286a41b, owner of kiln's P3) asks to be told when G's batch
  merges to main. P3 and kiln's unify/takeover landing wait on G. Ada restated today: "g then d". D is built after G,
  on its slot. My reply to Direct Routing was refused (kiln send backpressure); this file is that reply.
- Seam owner: G's code is AGREED b151af0..0030f95 (amphora-owner-t1-g-slot-0030f95-20261112), head to the batch;
  its final comes on the merged tree after the gates. Kiln Metrics agreed both event names (amphora-g-slot/events/v1), so the condition is met.
