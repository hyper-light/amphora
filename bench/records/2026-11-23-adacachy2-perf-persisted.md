# Lane W (persisted) on adacachy2: "the database advanced since preparation" (2026-11-23)

Ada's goal is Amphora correct and about 10x faster than every competitor. Bench lane S4 saw the persisted class refuse
writes as `Invalid { what: "prepared transaction", why: "the database advanced since preparation" }` at 128 writers and
in its 5-kill failover smoke, until the bench's 10 s deadline. This lane looked for why and fixed what it found.

## What the evidence says

1. **Not persisted alone.** S4's own round logs (`2026-11-23-adacachy2-repl/round*.err`) show the replicated class
   failing the 128-writer rung with the same refusal in 3 rungs, and one persisted failure that was a `Timeout`. The
   class is not the cause.
2. **Not stuck.** The unmodified bench (7ac4153d, S4's binary) at 128 writers on persisted passed when this lane ran it
   (r630, load 71-81): 24.8 writes/s, p50 3.6 s, p90 6.4 s, max 9.2 s against the 10 s deadline. Every attempt waits
   seconds in its member's queue, so a write refused once rarely fits a second attempt inside 10 s. The refusal is
   what the bench reports because it is the last error a timed-out writer saw.
3. **Where it is raised.** `replication::on_base` (crates/amphora/src/replication.rs): a prepared write applies only on
   the entry it was prepared after. Nothing keeps a stale preparation across retries: each retry is a new call,
   prepared again. Whether a restart leaves anything stale is not settled here: the driven crash test (32 writers,
   five leader crashes, on branch perf/persisted-crash) stops first on `Full { limit: 2 }`, a restarted member's
   input bound, which the test pump's `admit_receive(..).unwrap()` and the test's writers don't yet take.
4. **The defect found: the leader's own writes never took the read-set check.** A leader prepares on its applied state,
   behind the forwards its log holds uncommitted (the persisted class's widest gap, its commit waiting on a quorum's
   flush). Members' forwards get the read-set walk at append (`step_forward`, `rebase`); the leader's own proposal
   (`Actor::propose`) did not, so it was refused whenever a follower wrote, whatever it touched.
   Driven runs, three writers (leader and both followers), 15 commits each, 32 lists spread over pages:

   | arm | leader refused (3 seeds) | leader most in a row | followers most in a row |
   |---|---|---|---|
   | fix reverted (a68c4d17, r679) | 21 | 8 | 1 |
   | fix (277d6d3b, r679) | 1, 3, 1 | 1 | 1 |
   | fix after merging main (113bca75, r697) | 1, 2, 2 | 1 | 1 |

   Earlier probe on the base (481a621d, r634): the leader refused 11, 17, 11 times, 4, 5, 3 in a row.
5. **What the fix does not change: the bench's rows conflict.** Every bench write appends to one table (its rowid's
   rightmost leaf, page 1's size), so no read set can move it. On two hot lists the followers are refused once for
   about every commit (9 to 15 refusals for 15 commits, never twice in a row: the slot), and the leader sometimes
   twice in a row outside its turn (seed 1 in r697, seed 3 in r667). The 128-writer driven test refused 59 to 63 times
   in all, at most 3 or 4 for one write, before and after the fix alike. So the 128-writer bench rung still depends
   on one refusal fitting a second queue wait inside 10 s; the remedies are decisions (below).

## What changed (perf/persisted)

- `Actor::propose`: the leader's own write takes the same read-set walk as a member's forward, against its last entry.
- `Actor::prepare`: a claimed write reads the floor again inside its traced session, so a raise appended after its
  base conflicts in the walk. Before this the claim was checked in `read_db`, outside the read set, so a forward whose
  pages missed the raise's could be moved past it (since read sets, 2026-11-21); the leader's writes now take the same
  walk, so this closes it for both.
- Tests: `no_writer_is_refused_twice_in_a_row_when_every_member_writes`,
  `a_hundred_and_twenty_eight_writers_each_land_and_every_acknowledged_row_is_held`. The crash test
  (`writers_through_five_leader_crashes_each_land_and_every_acknowledged_row_is_held`) is unfinished, on
  perf/persisted-crash.

## Commands

All on adacachy2 (runner 00aad8da...), needs `linux/amd64,cpus=8,mem=24g`, CARGO_INCREMENTAL=0, CARGO_BUILD_JOBS=8,
target `~/.cache/amphora-perf-target/persisted`, a clone at `~/.cache/amphora-perf/persisted-remote` checked out at
each SHA (`~/.cache/amphora-perf/persisted-scripts/t.sh SHA cargo-args`).

- Bench repro: `~/.cache/amphora-perf/persisted-scripts/repro.sh BIN 128 procs` (three `--repl-member` processes on
  29390-29392, then `--repl commit --system amphora-procs --class persisted --writers 128 --batch 1 --text-bytes 600
  --deadline-s 10 --relay-records 128 --spanning 2 --warmup-s 1 --seconds 5`).
- Tests: `cargo test -p amphora --lib --locked -- --test-threads=8 --nocapture no_writer_is_refused_twice_in_a_row
  a_hundred_and_twenty_eight_writers`.
- Module suites: `cargo test -p amphora --lib --locked -- --test-threads=8 database::replicated replication::
  database::slot`. Before merging main (277d6d3b): 160 passed, 5 failed, each a known failure on main that lane
  replicated-K5 fixed (2026-11-23-adacachy2-perf-replicated.md). After merging main (113bca75): 165 passed, 1 failed
  (the crash test, since moved to perf/persisted-crash). At 6d0f897a: 165 passed, the crash test failed on
  `Full { limit: 2 }`.
- Merged to main as d4d0e096 (r749): clippy `-p amphora --all-targets -D warnings` clean; the touched tests and the
  floor, raise, read-set and slot tests 24 of 24; the module suites with `database::application` 164 passed, 1 failed:
  `a_change_past_its_deadline_answers_what_its_stage_knows`, main's own (lane replicated-K5, r683, the same
  `Full { limit: 2 }`).
- Not checked on the bench's own surface after the fix: `cargo build --release --locked --manifest-path
  bench/Cargo.toml` at d4d0e096 with the lane's offline CARGO_HOME refused (`no matching package named awak`, r759).
  The prediction, not measured: the 128-writer rung does not change, its rows conflicting on every write (point 5).
- Cost not measured: every claimed write now reads the floor policy once more inside its preparation (one small
  table read on the owner's thread).

Load on adacachy2 ran 60 to 1189 through these runs (other lanes); every number above is a count from driven time, not
a timing.

## Decisions for Ada

- **A typed refusal for a stale base.** Today it is `Invalid`, which says the caller's input was wrong; kiln retries
  it by matching the text. A new `Refusal` variant is a wire change (append-only, a new capability) that kiln and the
  bench would read. Choice: keep `Invalid` with its text, or add a retryable variant.
- **The leader's general tail rule** (replication.md, "The leader's turn"). Outside its turn the leader still prepares
  behind uncommitted forwards; on hot rows it can be refused twice in a row. The rule would make it wait for its log,
  as followers do, but two floor tests rest on a leader write prepared beside an entry in flight. Choice: adopt it and
  change those tests, or keep today's rule.
- **Hot rows under many writers.** A refused member's slot serves its next write, which under a deep queue is another
  caller's, so the refused caller's retry waits a whole queue again. Holding every member's barrier while a slot is
  held would end those refusals, but a barrier does not say whether it is a read or a write, so reads would wait on
  slots too.

# Lane W2 (persisted) on adacachy2: a retryable refusal for a stale base (2026-11-23)

Ada's goal: Amphora correct and about 10x faster than every competitor, and replicated writes never stuck. The
coordinator's decisions on lane W's three questions: (1) a typed, retryable refusal; (2) the leader's general tail rule
by an A/B; (3) read-or-write intent on member requests. Branch perf/persisted-w2.

## 1. `Refusal::Serialization` (done)

- `replication::on_base` now refuses a write whose base moved `Refusal::Serialization { what: "prepared transaction",
  why: "the database advanced since preparation" }`, appended to `amphora_api::Refusal` (PostgreSQL's 40001). Its
  Display is the `Invalid` text it had, so a caller matching the text still retries. Every match on the old
  `Invalid { what == "prepared transaction" }` (the slot's counts, the floor keeper, `overtaken`, the driven tests)
  matches the type now.
- Wire: the owner transport's `VERSION` 9 to 10 (`VERSION_SERIALIZATION`), the `Q` connection's `SQL_VERSION` 2 to 3
  (main had taken 2 for streamed answers). An owner sends the variant only on a connection that agreed on that version,
  and the `Invalid` it was below it (`transport::common::spoken`, `Refusal::without_serialization`).
  local-owner-transport.md, "Version 10".
- Bench: `send` asks a write refused `Serialization` again at the node that refused it and moves to the next node on any
  other failure; `--repl-member` sends the refusal's type beside its text (`"refusal"`), and the driver decodes it.
  Not compiled here: the bench's own manifest still refuses offline (`no matching package named awak`, lane W's r759).
- Tests: `an_overlapping_write_is_refused_serialization_and_a_malformed_one_invalid` (driven, three members appending to
  one table: some append refused `Serialization` with the old text, a follower's self-refused write `Invalid`, every
  append held once); `a_peer_before_version_10_is_sent_serialization_as_invalid` (at 9 the `Invalid`, decodable; at 10
  the type; a malformed request's `Invalid` unchanged at both).
- Checks: at e9204b04 (r838) clippy `-p amphora-api -p amphora --all-targets -D warnings` clean; the touched and
  neighbouring tests 31 of 31 (the two new ones, the 128-writer test, the stale-base raise, the slot and leader's-turn
  tests, the floor tests, transport version tests, `serve::`); `--test transport` 6 of 6 (r809).

## A finding from the first version of the test (r831): a slot held by a write that prepares nothing

Run beside the appends, a follower's write that refused itself (nothing prepared, nothing forwarded) left the next
member's append unanswered for good in the driven harness (round 1, the append via member 2). The slot core
(database/slot.rs) ends a holder's slot on its next forward, its second barrier, or its learned length passing. A
holder whose write prepares nothing (a self-refused write, a SQL error, or a read made through `call`) sends no
forward, so every other member's forwards and the leader's own writes wait out the slot's length: an election timeout
before any gap is learned. The driven clock stands still, so there it never ends; in production, inferred and not
measured, it costs up to that length per such write under contention. Item 3's intent on barriers would let a read
skip the slot; a write that fails in preparation still needs the member to say it has nothing to forward. Not fixed
here.

## 2. The leader's general tail rule (not started)

Not built: the hour went to item 1. Where it goes: the leader's turn rule (replication.md "The leader's turn",
`Slots::turn`, the turn's prepare-after-apply target in `Actor`), made to apply to every leader write behind one switch;
the two floor tests that rest on today's rule are `a_raise_beside_a_change_holds_through_a_lost_lead` and
`a_raise_whose_base_is_joint_is_refused`. The A/B's bench rungs need the bench to build on adacachy2 first (offline
`awak`).

## 3. Read-or-write intent on member requests (not started)

What it touches, read here: a member's barrier is a raft ReadIndex whose context is `Actor::context`'s four `u64`s
(id, term, incarnation, serial; `CONTEXT_BYTES`), the same bytes the proposal's log entry carries. Appending an intent
byte changes (a) `CONTEXT_BYTES`, which `FRAMING_BYTES` and so `MESSAGE_BYTES` derive from (a record bound peers
check), (b) the follower's match of a ReadState to its pending context, and (c) the leader's `Slots::read`, which gets
the message whole. The kinds known at the barrier: `Operation::Mutation` and `Import` are writes, `Get`, `Scan`,
`Changes`, `Head` reads, `Call` unknown (a closure; a write unless its caller says otherwise).

## 4. The crash test after lane K6's fix

K6's fix is on main (46abb6c0). bd6abd42 had taken the test off main; W2 brought it back on main 7777fa99 (with
`Serialization`), perf/persisted-w2-crash 5159e10e. r882: it passes, "32 writers through 5 leader crashes: 25 sent
again, 153 rounds", one run (load 14-34); clippy `-p amphora --all-targets -D warnings` clean (the run after r882). No stale state
after a restart showed in this run; it stopped on `Full { limit: 2 }` before K6's fix and not after. The other W2
checks after the merge (09f19c51, r861): clippy clean, the touched tests 36 of 36, `--test transport` 6 of 6. Not
run: the module suites on main after the push (rule 5b), for the successor.

# Lane W3 (persisted) on adacachy2: floor raises answer Serialization; a failed slot holder says so at once (2026-11-23)

Ada's goal: Amphora correct, about 10x faster than every competitor, and at agentic Meta scale one failing or slow
client never stalls everyone. The coordinator's rulings on W2's questions, in order. Branch perf/persisted-w3.

## 1. A floor raise refused for a moved base answers `Serialization` (done)

`writer_floor` passed the stale-base rule's refusal through `overtaken`, which turned it into `Unavailable` ("the
group changed while the raise was proven; raise again"). Deleted: the raise is answered `Serialization`, like any
write whose base moved, still counted `FloorRefusal::StaleBase`. Its text is the `Invalid` text it had, so a caller
matching it still retries; a peer below transport 10 / SQL 3 is sent that `Invalid`. replication.md, "Raising the
floor" b. The driven tests `a_raise_a_change_overtakes_is_refused_by_the_stale_base_rule` (Serialization) and
`a_raise_another_lead_wrote_over_is_answered_unknown_outcome` (UnknownOutcome) lose their assertions on the deleted
mapping. The driven tests reach the raise through the owner's queue, not `writer_floor`, so `writer_floor`'s
pass-through is checked by its having no mapping left, not by a test.

## 2. A slot holder whose write forwards nothing tells the leader at once (done)

**The ruling asked for a new message at the end of the wire; this uses one every leader already reads.** The
members' wire is hyper-raft's record format (vendor/hyper-raft `wire.rs`: a version byte, a kind byte, unknown kinds
refused): Amphora has no member-to-member envelope of its own and no version agreed between members. A new kind
there is a vendored hyper-raft change. But the slot core already ends a holder's slot, `released`, on its second
barrier before any forward (`Slots::read`, slot.rs since amphora-owner-t1-g-slot-878285a): every leader, old or new,
reads a second barrier as "my slot's work was no write". What was missing was a member that sends it.

- `Actor::slotted`: the term in which one of this follower's writes was refused `Serialization` (learned from its
  own answer, on the commit or at apply: the verdict the leader queued it by at its append).
- `Actor::prepare`: when the barrier's work ends with nothing forwarded (a read through `call`, a claim or SQL
  refusal, the deadline) and `slotted` names the current term, the member sends a fresh barrier (`replica.read`
  with a new context). The leader ends the slot on its next step. The barrier's read state matches nothing at the
  member and is dropped (`output.reads`). A forward clears `slotted` too.
- Cost, not measured: one ReadIndex (the leader's heartbeat round to a quorum) per write that forwards nothing right
  after its member was refused. Every other write sends nothing new.
- Test `a_slot_holder_whose_write_fails_ends_its_slot_without_waiting_its_length`: three members; the holder, queued
  and knowing it, writes to a missing table; then the leader and member 2 write. Both commit, the holder's slot
  began, and `Driven::lapses` is 0: the harness never moved a clock past a slot's end. Red arm f5a2408b (the same
  without the barrier, branch perf/persisted-w3-red): the test fails (r923).
- replication.md, "When a slot starts".

## 3. The leader's general tail rule A/B (not started)

Not run: the hour went to items 1 and 2, with adacachy2 refusing runs for room (0 to 3 idle cores) for the first 20
minutes. Where it stands is W2's note, unchanged.

## 4. Read-or-write intent on barriers (not started)

## Checks

All on adacachy2 (runner 00aad8da...), needs `linux/amd64,cpus=8,mem=24g`, CARGO_INCREMENTAL=0, through
`~/.cache/amphora-perf/persisted-scripts/t.sh SHA cargo-args`.

- r931 at c5e4ba73 (load 54-68): clippy `-p amphora --all-targets --locked -D warnings` clean;
  `a_slot_holder_whose_write_fails...`, `a_raise_a_change_overtakes...`, `every_falsifier_of_the_floor_holds_under_a_nemesis`
  3 of 3; at base 9a7b14bb the nemesis test passed too.
- r923 at 167cc8f9 (item 2 before the clippy fix, load 36-78): 37 of 38 of the floor, slot, raise, retry and
  overlapping-write tests; `every_falsifier_of_the_floor_holds_under_a_nemesis` failed once, at its promote-while-the-
  tail-is-unapplied assertion (replicated.rs, "At once, Unavailable, naming both indexes"), and passed in r931 on the
  same code. Seen once in three runs (two with this change, one without): not settled whether this change or the
  test's own timing (its flushes run off the driver's thread) caused it. For the successor: run it 10 times on both
  arms. Red arm f5a2408b: the new test failed, as it should.
- r951 at b3b4b6d8 (merged with origin/main 952f5766, load 55-128): clippy `-p amphora --all-targets --locked -D
  warnings` clean; the floor, slot, raise, retry, overlapping-write and new tests 38 of 38 (the nemesis among them).
- Not run: the module suites after the merge (rule 5b), and no timing of the extra barrier.
