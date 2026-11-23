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
