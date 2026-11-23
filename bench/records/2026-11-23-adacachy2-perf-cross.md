# perf/cross: the memory budget's reading per request (AUDIT 2026-11-23 §1)

Host: adacachy2 (runner 00aad8daff2fa3c9daa65d53c425d450, linux/x86_64, 32 cores), shared with the
rest of the swarm (the scheduler reported 28 of 32 cores idle at the "before" run, 1 of 32 at the
"after" run: the "after" numbers ran on the busier machine). Release profile of the workspace.

Command (each label built from its own tree, then run three times):

    cargo build --release --locked -p amphora --example admission
    admission <dir> 200000 3 1,4,16,64,256,1024

`room`: `memory::process().room(Use::Queues)`, ns a call, best of five rounds of 200,000 calls: what
every `Database` request's admission pays (`Admission::reserve` -> `Budget::take` -> `room`).
`call`: `Database::call` of an empty closure, calls a second over 3 s after a 1 s warm-up, at each
client count (one thread a client). No call was refused in any run.

## Verified

The audit's claim holds: every admission read `/proc/self/cgroup`, opened and read each group's
`memory.max` up the hierarchy, `/proc/meminfo` and `/proc/self/statm`, plus three `getrlimit`s:
57-70 us a call, and serialised enough in the kernel that throughput fell from 16 clients on.

## Change

Linux's readings come from files opened once and read again in place (`pread`) at every ask:
`/proc/self/cgroup` (the groups' files opened again when it changes: the process was moved), each
group's limit file, the `cgroup.controllers` of each v2 group without a `memory.max` (opened again
when its parent enables the memory controller), and `/proc/self/statm`; the machine's memory from
`sysinfo` (`totalram`, the count `/proc/meminfo`'s `MemTotal` prints). Every reading is the
kernel's at the ask: nothing is cached across asks but open files.

| | before (3 runs) | after (3 runs) |
|---|---|---|
| room, ns/call | 57,093 / 69,777 / 64,025 | 2,836 / 3,292 / 3,569 |
| call/s, 1 client | 6,682 / 3,488 / 2,571 | 37,963 / 22,489 / 8,887 |
| call/s, 4 | 18,879 / 7,092 / 12,888 | 54,114 / 73,355 / 53,229 |
| call/s, 16 | 35,031 / 12,349 / 27,785 | 55,530 / 51,045 / 66,789 |
| call/s, 64 | 18,165 / 13,173 / 19,135 | 52,914 / 59,768 / 59,001 |
| call/s, 256 | 3,669 / 6,915 / 6,070 | 56,323 / 61,200 / 59,811 |
| call/s, 1024 | 910 / 2,322 / 1,539 | 64,816 / 59,066 / 60,406 |

room: 17-25x cheaper. At 256 clients 8-17x more calls a second, at 1024 26-71x; the ladder no
longer falls off past 16 clients. The single-client rows vary 4x run to run on the shared host.

## The flat 32 MiB charge per request (perf-A2, AUDIT §1)

Verified: every local request charged `REQUEST_MAX` (32 MiB) of the room for queues whatever it
was, so a process under a 1 GiB limit refused past ~30 requests in flight (16 GiB: ~500).
Changed: `Admission::bytes_for` charges a request's command and answer, the bytes it carries and
its answer's bound by the API: a Records get one row (KEY_MAX + VALUE_MAX, ~1 MiB), a Records
write what it carries, a head / compaction step / holding a fixed answer. SQL, scans, changes,
imports and calls keep `REQUEST_MAX` (no tighter answer bound; a call's answer is its closure's).

Command (adacachy2, each label built from its own tree; scope from `systemd-run --user --scope -p
MemoryMax=1G`; `perfx/cgroup.sh <label> 1G get`):

    admission <dir> 1000 2 16,64,256,1024 get

Records gets of one absent key a second / refused a second, two runs each:

| clients | before (32 MiB a get) | after (~1 MiB a get) |
|---|---|---|
| 16 | 65,237 / 0; 65,209 / 0 | 87,971 / 0; 112,392 / 0 |
| 64 | 62,748 / 278,832 refused; 56,781 / 212,703 | 101,367 / 0; 118,337 / 0 |
| 256 | 63,143 / 282,569; 58,704 / 225,030 | 118,749 / 0; 116,703 / 0 |
| 1024 | 61,215 / 280,265; 57,213 / 201,224 | 116,728 / 266,274; 111,357 / 259,398 |

Under 1 GiB, refusals began between 16 and 64 in flight before, between 256 and 1024 after (the
1024 x ~1 MiB the gets now ask passes the room: refused typed `Full`, the budget never overrun).
Admitted gets a second ~1.8x (no client spends its turn being refused). The scope's limit held:
no run was OOM-killed.

## The db:: hang (perf-A5)

Named: `db::tests::constants_22_a_busy_write_wakes_on_the_write_locks_release`. Every other test
whose name holds `db` passed on both trees. Run r484 (adacachy2, needs cpus=8,mem=24g): one test
binary per tree, each test alone (`--exact`, `--test-threads=1`) under `timeout 120`:

| tree | constants_22 | the other db tests |
|---|---|---|
| perf/cross a913aa42 | timed out (124) | 9 passed |
| main 62687f5d (its merge base) | timed out (124) | 7 passed |

The branch does not cause it: main hangs the same way, and a third copy from T1's land-noqb
build (lib test binary of 2026-11-22 16:14 PST, before A3's waker commit) had spun 12 h 15 m at
98.7% CPU on adacachy2 when looked at: its test thread waiting on a futex, one thread it
spawned running (utime 43,524 s). The test steps with a waker of its own, not the per-thread
`UNPARK`, so A3's waker reuse is not on its path.

Cause (lane constants-22's `LockRelease`, crates/engine/busy.rs): a busy statement reads the
log's release count before it tries its locks and, when busy, waits only if the count has not
moved. A write busy on another connection's write lock ends the read transaction its failed
upgrade began (`Insn::Transaction` on `Busy`, execute.rs), and that unlock notifies the same
`LockRelease`: the count moved by the waiter's own release, `wait` said "a release came, try
again now", the step returned `Sleep { 0 }`, and the next try did the same, for as long as the
other connection held the lock. In the test the holder commits only once the waiter parks, so
the two never finish.

Fix (2286cfb9): a step counts its own releases of the log it may wait on (a thread-local count
keyed by the log's `LockRelease`, begun where the step reads the count), and `wait` waits when
the count moved by exactly those: `generation == seen + own`. Another connection's release
still ends the wait at once. A step inside a step on the same log shares the count; one on
another log counts its own and gives the outer's back when it ends (an outer that lost its
count only tries once more, never misses a release).

The waker reuse (A3) checked for soundness: the shared `Unpark` wakes the thread a fresh one
would, a stale wake only ends a park early and the loop tries the step again, and
`LockRelease::wait` holds a waker once by `will_wake`, which one waker per thread keeps
exact. Kept.

Test that fails on the hang: `busy::tests::a5_a_steps_own_releases_never_end_its_wait` (the
old `wait` says try again after the step's own release).

Checked on 033edffd (the fix merged with main 486edca0), run r558 on adacachy2 (needs
cpus=8,mem=24g; load 20-36 before, 18-33 after): `cargo clippy -p amphora-engine -p amphora
--all-targets --locked -- -D warnings` clean; `cargo test -p amphora-engine --lib busy::` 16
passed (the new test among them); `db::tests::constants_22_...` alone passed in 0.34 s,
printing `wake=Release commit_to_done=588.852µs woken_before_release=Some(0) wakes=2` and
`wake=Schedule commit_to_done=1.284473ms woken_before_release=Some(1) wakes=2` (before the fix it
never finished); `cargo test -p amphora --lib db` 10 passed, the allocs tests among them. The
wake latencies are one run each on a shared host, printed by the test, not asserted.

## Scans and SQL batches charged their real worst case (perf-A5, on perf/cross, NOT merged)

`Admission::bytes_for` on perf/cross: a scan `(limit + 1) x (size_of Row + KEY_MAX + VALUE_MAX)`
plus its family, prefix and after; an SQL batch its statements' text and parameters byte for
byte, a `SqlResult` envelope per statement, and `RESULT_MAX` for the rows. Calls, migrations and
imports keep `REQUEST_MAX`. Test `database::admission_charge::mb1_...` asserts each.

Checked on 37eeaef9 (r569): clippy on amphora clean; the database:: suite 217 passed, 7 failed.
The same 7 fail alone (`--exact`, `--test-threads=1`) on main 06c77548 (r577) and on main
486edca0, before the hang fix landed (r601), so none is this change's or the hang fix's:
`patience::tests::a_joint_configuration_takes_the_larger_of_its_two_sets` (`Full { voter: 5,
limit: 3 }` at patience.rs:418) and six `replicated::tests` (`a_follower_retry_racing_...`,
`t_qb_1_9_...`, `every_falsifier_of_the_floor_holds_under_a_nemesis`,
`a_host_waits_for_a_leader_...`, `a_member_installed_from_a_snapshot_...`,
`compaction_waits_for_the_engines_flush`).

Not merged, for a decision: a full page's worst case is real (no byte bound stops a scan page:
1,000 rows of 1 MiB values are ~1.0 GB), but charged at admission it refuses every default-limit
scan where the queue room is under ~1 GB (a 1 GiB process with 100 MiB resident has ~924 MiB of
room), where today the same scan is admitted at 32 MiB. The alternative that keeps scans working
there: end a page at a byte bound too (`RESULT_MAX`, `more = true`), which makes its real worst
case ~32 MiB plus one row.

## A page ends at RESULT_MAX bytes, and is charged that (perf-A6, the coordinator's decision)

The alternative above, taken: a scan's page and a page of the changes feed end at their limit's
rows or at `RESULT_MAX` (32 MiB) of keys and values, whichever comes first (`database::fits`),
and say more follow: a scan by `more = true`, the feed by its head past its last revision (a
page still never cuts a revision). The row past a page is weighed in place (`blob_len`), not
copied. Across groups the same: each group's page ends by the rule, the union stops at the
smallest last key of a group that has more, then ends by the rule; the feed across groups cuts
its answer on a stamp boundary (`across::whole`).

Charge (`Admission::bytes_for`): a scan its family, prefix and after, plus
`limit x size_of Row + min(limit x (KEY_MAX + VALUE_MAX), RESULT_MAX)`; the changes feed the
same with `Change`. A default-limit scan (1,000) is charged about 32 MiB + 1,000 envelopes, not
~1 GiB, so it is admitted wherever it was before. A page also fits the wire's frame
(`RESULT_MAX + 1 MiB`), which 1,000 rows of 1 MiB values did not.

Test `database::admission_charge::a6_a_page_ends_at_result_max_and_continuing_reaches_every_row`:
40 rows of 1 MiB values (five transactions of eight); scanned at limit 1,000 the first page ends
early with `more`, every page carries at most `RESULT_MAX`, and continuing reaches all 40 keys
once, in order; the feed's pages hold whole transactions only and reach all 40. `mb1_...` asserts
the charge equals the bound.

Merged to main as be247a75 (merge of origin/main ce73cba1, which brought the replicated test
fixes). Checked on be247a75 (r687, cpus=4,mem=16g, load 180 before, 89 after): clippy clean;
database:: 225 passed, 1 failed, the known patience test
(`a_joint_configuration_takes_the_larger_of_its_two_sets`). After the merge (r696, load 490 to
1189): `tests/database.rs` 9 passed; `tests/records_conformance.rs` 5 passed and
`the_records_suite_answers_alike_at_one_voter_and_three` failed ("one voter and three answered
differently"); alone (r716, load 833 to 908) it passed on be247a75 and on main ce73cba1, so it is
the host's load (an admission A/B of this lane's had ~8,400 threads up then, since stopped), not
this change. Its pages are far under RESULT_MAX, where the new rule cuts exactly as the limit did.

## The call charge's A/B (perf-A6, branch perf/cross-call at fbf7dbdb, NOT merged: no winner)

Built: `Database::call_charged(until, AnswerCharge, f)`, `AnswerCharge::Declared(max)` (arm a)
or `AnswerCharge::Measured` (arm b), answers sized by a new `AnswerBytes` trait required only
there; `call`, `call_session`, `query` and the remote call are unchanged. Both arms check the
answer's size inside the call's transaction, before `connection.finish()`: past its bound (a) or
past the budget's room (b) the call is refused and rolled back (test
`tests/database.rs::mb_1_a_charged_call_past_its_declared_answer_rolls_back`). Checked at
62c605cc (r737): clippy clean, `tests/database.rs` 10 passed.

`admission <dir> 1000 5 4,8 ab 5` (r737, needs cpus=8): arms flat (today's `REQUEST_MAX`),
declared and measured alternated in each of 5 rounds of 5 s; each call `SELECT id, v ... LIMIT 8`
over 1000 rows; every call timed. Load 465 falling to 94, CPU pressure avg10 0.5-51%. Median
[range] of 5 rounds, microseconds:

| arm | workers | p50 | p99 | p99.9 | calls/s | refused |
|---|---|---|---|---|---|---|
| flat | 4 | 60 [57-81] | 450 [132-1204] | 804 [383-2432] | 37363 [24922-63370] | 0 |
| declared | 4 | 59 [57-96] | 936 [248-1037] | 1702 [568-1931] | 29196 [27494-55693] | 0 |
| measured | 4 | 75 [71-76] | 672 [124-1133] | 1204 [331-2004] | 32261 [21506-52903] | 0 |
| flat | 8 | 123 [110-197] | 884 [182-1453] | 1638 [443-2950] | 33842 [33030-69334] | 0 |
| declared | 8 | 118 [113-121] | 1070 [234-1361] | 1593 [504-2648] | 35291 [32167-65920] | 0 |
| measured | 8 | 154 [146-202] | 1330 [253-1437] | 1953 [470-2273] | 25575 [23672-52023] | 0 |

Verdict: inconclusive. Nothing was refused, so the charge never bound admission at 4-8 calls in
flight. Declared and flat are alike within noise; measured's p50 is about 25% higher in each round
at both worker counts (a guess, not profiled: its second budget take, on the writer thread, for
every call). The tails' ranges overlap in every arm on a host this loaded.

1k and 10k calls in flight were not measured: `Database::call` blocks until its answer, so each
call in flight is an OS thread. The 10k-thread runs (r661, r685) drove adacachy2 past load 1000
and were discarded at the coordinator's word. What they showed, unvetted (the OS scheduler was the
likely limiter): at 10k, flat refused about half the calls as Full and kept 36-43k calls/s, while
declared and measured admitted every call and fell to 120-900 calls/s with p50 7-12 s. Measuring
1k/10k fairly needs a call submitted without blocking (a submit that answers through a callback,
as the writer's `put_then` already does inside), so a core-sized pool keeps that many queued.
