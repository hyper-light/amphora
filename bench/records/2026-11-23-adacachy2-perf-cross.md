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
client count (one thread a client). The harness recorded zero refusals per second, rounded to
the nearest integer. One refusal during a three-second arm also rounds to zero. Exact refusal
counts for these historical runs are unavailable.

The revised example emits exact `completed` and `refused` totals for calls begun in the
measured interval. It also emits `unexpected_errors` and fails if any refusal was other than
admission `Full`. A successful GET must return the fixture's absent key.

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

## The call charge at 1k and 10k calls in flight (perf-A7, NOT decided: mixed)

`Database::call_nowait(until, AnswerCharge, f) -> Result<Calling<R>, Refusal>` (on main since
bc3f2b9d): a charged call admitted and queued without blocking; its answer reaches the returned
`Calling` (a `Future`) on the store's notifier thread, waking its task. Refused at submit, typed:
the budget's room, or a full writer queue (`Full`, where `call` waits for room). `AnswerCharge::Flat`
is today's `REQUEST_MAX` as the control. Tests (tests/database.rs, tests/call_nowait_charge.rs):
`a7_a_nowait_call_answers_exactly_once`, `a7_a_refused_or_oversize_nowait_call_is_typed_and_rolls_back`,
`a7_a_dropped_calling_gives_back_its_charge`; r883: clippy -p amphora --all-targets clean, 13 passed.

Command (r879, adacachy2, needs cpus=16,mem=48g, release, binary admission-4535a304):

    admission <dir> 1000 3 1000,10000 nowait 5 16

16 pool threads keep N calls in flight (a call ends, the pool submits the next; a refused submit
waits for the next answer); each call `SELECT id, v ... LIMIT 8` over 1000 rows; latency from
submit to answer, every call timed; arms alternated in one process, 5 rounds of 3 s after 1 s
warm-up. Load 45 rising to 92 on 32 cores, CPU pressure avg10 34-50%. Median [range] of 5 rounds:

| arm | in flight | p50 ms | p99 ms | p99.9 ms | calls/s | refused at submit / 3 s |
|---|---|---|---|---|---|---|
| flat | 1k | 15.7 [14.8-19.3] | 48.8 [37.2-83.0] | 59.2 [39.9-101.6] | 48,704 [43,463-55,772] | 0 |
| declared | 1k | 16.1 [14.3-24.7] | 87.2 [39.6-104.3] | 95.7 [47.7-162.5] | 46,992 [30,784-55,824] | 0 |
| measured | 1k | 19.0 [18.5-34.6] | 66.6 [51.7-82.1] | 75.8 [58.9-92.1] | 33,074 [25,527-42,793] | 0 |
| flat | 10k | 62.1 [58.6-84.3] | 137.8 [120.6-399.7] | 144.0 [127.5-423.3] | 53,245 [32,425-55,113] | 13,241 [6,928-16,341] |
| declared | 10k | 90.7 [68.0-232.0] | 233.5 [145.3-447.9] | 242.0 [152.2-455.8] | 45,054 [19,455-54,783] | 11,938 [4,430-17,030] |
| measured | 10k | 86.5 [84.3-232.1] | 344.9 [183.0-414.9] | 366.3 [191.6-427.8] | 37,167 [19,626-52,904] | 7,912 [3,608-11,984] |

No call failed or was refused after admission. Verdict: mixed, no winner. Flat and declared are
alike at 1k p50; every tail range overlaps. Measured is slower at 1k in every round's p50 and in
throughput (-32% median), A6's sign again; not profiled, so its cause (the second budget take on
the writer) is still a guess and was not moved. At 10k the arms did not hold 10k in flight: every
arm was refused at submit, and by Little's law (p50 x calls/s) about 3-5k were in flight, so the
10k rows measure the writer's queue (store `Config::queue`, 4,096 messages), not the charge. The
bench counts `Full` at submit without its cause (budget room or queue), so it can't say that the
charge never bound; on a host with ~98 GB free it most likely did not. The charge's purpose,
admitting more under a memory limit, was not exercised (no `MemoryMax` scope this run).

## The writer's queue has no count; the call charge at 1k and 10k truly in flight (perf-A8)

**Unit 1, the queue's 4,096 removed (on main).** The writer's queue held 4,096 messages
(`store::Config::queue`, a picked count: "a channel of this bound takes its slots at once ... so it
needn't be deep"), and `call_nowait` refused `Full` past it. A database already charges every
request to the memory budget's room for queues at admission (MB.1), which bounds its queue in
bytes; the count refused calls the budget had room for (A7's 10k rows). Removing the count beats
waiting for room under it: there is no second bound to wait on, so nothing polls (`ROOM_LOOK`, the
5 ms re-look, now serves only counted queues: `Groups` and the store's own tests, which keep
`Some(4096)` and `Some(1)`). `Config::queue` is `Option<usize>`; a `Database`'s store passes
`None` (an unbounded `mpsc::channel`, its blocks allocated as used). A queued call is still bounded
by its deadline: the writer answers `Timeout` for a call whose `until` passed before it ran.
Test `tests/database.rs::a8_ten_thousand_nowait_calls_in_flight_are_all_answered_none_refused_for_queue_room`:
the writer held by one call, a pool of one thread a core queues 10,000 `call_nowait`s behind it
(the old bound refused the 4,097th), all 10,000 answered with their own value. Memory refusal stays
typed: `a7_a_refused_or_oversize_nowait_call_is_typed_and_rolls_back`. r921 (b01eb1e7): clippy -p
amphora --all-targets clean; tests database 13/13, store 15/15, call_nowait_charge 1/1, lib store::
4/4.

**Unit 2, refusals by cause.** The `nowait` bench counts refusals at submit and through the answer
by cause: `memory` (`Full`, now with one source, the budget's room for queues), `deadline`
(`Timeout`, `UnknownOutcome`) and `other`; there is no queue cause left to count. Each line also
records memory pressure (system and the process's cgroup), the cgroup's `memory.current` and
`memory.max`, and the budget's room for queues.

**Unit 3, the A/B (r949, adacachy2, needs cpus=16,mem=48g, release binary admission-b01eb1e7;
on main as 5703a869, whose later commits only reformat the call path).** `admission <dir> 1000 3
1000,10000 nowait 5 16`: 16 pool threads, arms alternated in each of 5 rounds (3 s windows after
1 s warm-up), once unconstrained, then once inside one `systemd-run --user --scope -p
MemoryMax=1G` (the bench read `memory.max` 1,073,741,824 in every line). One shared
`XDG_CACHE_HOME` for both runs (RULES 5e). Load 51-137 on 32 cores (other lanes' work), CPU
pressure avg10 5-70%, I/O 2-15%, memory 1-4%; the scope's own memory pressure 0.00 throughout.
"in flight" is Little's law (p50 x calls/s), median of rounds. Median [range] of 5 rounds:

| cond | arm | offered | in flight | p50 ms | p99 ms | p99.9 ms | calls/s | refused (memory, at submit, per 3 s) |
|---|---|---|---|---|---|---|---|---|
| free | flat | 1k | 857 | 20.2 [14.9-22.4] | 70.0 [17.1-112.1] | 78.3 [19.0-116.0] | 40,467 [27,527-67,722] | 0 |
| free | declared | 1k | 794 | 20.0 [15.7-23.9] | 73.8 [56.8-89.7] | 83.7 [63.0-103.3] | 39,767 [30,231-47,979] | 0 |
| free | measured | 1k | 822 | 32.4 [24.7-61.4] | 94.1 [71.8-257.1] | 114.2 [85.6-269.0] | 26,936 [11,420-33,251] | 0 |
| free | flat | 10k | 2,950 | 76.5 [58.6-109.3] | 241.4 [72.8-623.8] | 250.9 [74.6-648.2] | 38,568 [20,793-67,068] | 18,020 [2,866-39,104] |
| free | declared | 10k | 7,969 | 245.0 [199.3-324.3] | 720.0 [356.0-1,058] | 735.6 [362.1-1,101] | 37,032 [24,574-41,505] | 0 |
| free | measured | 10k | 7,422 | 276.1 [193.1-656.2] | 853.7 [461.0-949.4] | 878.1 [470.5-970.3] | 28,158 [14,944-38,227] | 0 |
| 1G | flat | 1k | 26 | 0.6 [0.5-0.8] | 2.1 [1.0-2.8] | 4.3 [1.5-5.3] | 40,875 [27,693-55,042] | 97,800 [58,897-147,677] |
| 1G | declared | 1k | 877 | 18.9 [15.1-31.8] | 36.3 [19.0-84.3] | 43.4 [22.1-91.5] | 50,820 [26,065-66,198] | 0 |
| 1G | measured | 1k | 881 | 26.9 [18.9-36.5] | 65.5 [25.0-121.4] | 68.4 [25.8-152.9] | 32,295 [24,066-50,901] | 0 |
| 1G | flat | 10k | 22 | 0.6 [0.5-0.7] | 2.4 [0.8-3.6] | 5.1 [1.1-10.0] | 35,387 [30,103-60,735] | 86,338 [63,984-164,540] |
| 1G | declared | 10k | 9,994 | 215.7 [151.3-324.2] | 470.1 [164.0-512.5] | 493.8 [165.1-525.5] | 40,575 [29,853-66,045] | 0 |
| 1G | measured | 10k | 9,950 | 273.7 [182.9-329.8] | 298.6 [198.1-626.9] | 300.8 [199.3-633.3] | 36,350 [26,792-54,610] | 0 |

No refusal had a deadline or other cause, and none came through an answer, in any arm. Losses
first:

- **Flat loses admission.** Its 32 MiB charge per call held at most ~3,930 calls in the budget's
  132 GB room unconstrained (2,950 in flight at 10k, 18k refused per 3 s) and 32 in the 1 GiB
  scope (22-26 in flight, 86-98k refused per 3 s). Its low latency at 1G measures those ~25
  admitted calls, not the offered load: refused callers wait outside the timed region.
- **Measured is slower than declared at p50 and in calls/s in all four cells** (p50 +62%, +13%,
  +42%, +27%; calls/s -32%, -24%, -36%, -10%), A6's and A7's sign again. At 1k unconstrained
  the p50 ranges don't overlap; elsewhere they do.
- **Measured's tails are lower at 10k in the 1 GiB scope** (p99 299 vs 470 ms, p99.9 301 vs
  494 ms by median, ranges overlapping). Every other tail favours declared, also overlapping.
- **The scope was short only for flat.** Declared and measured charge ~1-2 KB a call: 10k of them
  are ~20 MB, and the scope's `memory.current` stayed 17-35 MB of 1 GiB. So the case where the
  charge decides who gets in between declared and measured (answers near the room) is still not
  exercised; it needs a smaller scope or larger answers.
- **Limiter (inferred, not profiled):** calls/s is ~27-51k in every arm at both 1k and 10k, and
  p50 ≈ in flight / calls/s (10k / 40k/s = 250 ms), so latency at 10k is queueing behind the one
  writer; the charge moves p50 through the writer's throughput (measured's second budget take runs
  on the writer). A busy host (load 51-137) adds round-to-round spread.

Verdict: no arm wins every cell. Declared wins p50 and calls/s everywhere and the tails in three
of four cells; measured wins the 10k tails under the 1 GiB scope, within overlapping ranges. Flat
is out of the running on admission. Not decided (Ada's call by data); no arm deleted.

## The call charge is the declared bound alone (perf-A9, Ada's decision on A8's A/B)

**Unit 1.** `Database::call`, `call_session` and `call_nowait` take the caller's `AnswerMax<R>`: charged at
admission, and the answer's bytes (`AnswerBytes`) checked inside the call's transaction before it commits, past
it refused `TooLarge` and rolled back. `AnswerCharge::{Flat, Measured}`, `call_charged`, the measured answer's
second budget take on the writer and the arm switch are deleted; `AnswerBytes` stays (the declared check needs
it). Every caller in the workspace declares, none by a picked number: `AnswerMax::fixed()` for a heap-free
answer (its type's size; the type system refuses `fixed()` for a `String` or `Vec`, `FixedAnswer`), the
table's SQL `size_of::<String>() + SQL_MAX`, an exact `Vec` its length's bytes, an SQL write's frames over the
wire and test row lists `RESULT_MAX` (the API's bound on a result, as an SQL batch is charged, lane perf-A5).
The codemod is `~/.cache/amphora-perf/cross-a9-scripts/declare.py`. The `admission` example keeps its `ab` and
`nowait` modes with the declared arm alone. Decision record: docs/design/store.md, "Decision (lane perf-A9)".

Checks (adacachy2, needs cpus=8,mem=24g, load 64-96 on 32 cores): r1105 at 091c2025, tests database 13/13,
call_nowait_charge 1/1, store 15/15, transport 6+1+1+1 passed, online_schema 10/10, lib `serve::` 11/11; lib
`sql::` 25/27, the two failures `xgroup::sql::sessions::t_t1_10_w7_*` and `t_t1_10_w11_*` (`query_waiting` on a
replicated reader, no `Database::call` on their path; see below). Clippy at 091c2025 found two `without_rowid`
row lists declared `fixed()`, fixed in 54a8bdf7.

What is left (not this unit): the cross-group steps' `call_internal` and a replicated owner's command queue
still charge a call `REQUEST_MAX` (replication.md, "A call is charged its worst case"); the bound would travel
in `Operation::Call`.

## A9's bench edits build; the three sessions tests are not A9's (perf-A10)

**Unit 1, bench builds on main.** r1235 (adacachy2, needs cpus=8,mem=24g, load 23): at main 5e6c619f,
`cp Cargo.lock bench/Cargo.lock` (bench/.gitignore keeps that copy out of git) then `cargo check
--all-targets` in `bench/`: rc 0, no errors. A9's `AnswerMax` edits in bench/src/{repl,sqlapi,scale,cluster}.rs
compile as they are; nothing to fix. Script: `~/.cache/amphora-perf/cross-a10-scripts/u12.sh`.

**Unit 2, xgroup::sql::sessions::t_t1_10_w7, w8 and w11 alone, 6 runs each.** Main is 5e6c619f (r1235, through
`cargo test --lib <name> -- --test-threads=1`); before A9 is 7c303299, the parent of A9's first commit
(r1245, the lib test binary run directly, one process a test). Both runs shared the lane's one target dir, and
the two commits' lib test binaries have one path there, so r1235's `cargo test` rebuilt main's over r1245's
binary after r1245's first runs: only runs whose log says `449 filtered out` (7c303299's test count; main's is
452) are counted for 7c303299. r1265 re-runs w7 and w11 five more times each from a copy of 7c303299's binary
(`~/.cache/amphora-perf-bin/cross/amphora-lib-7c303299`, logs `a10-pre2-*` on adacachy2); its numbers are in
the r1265 table below (lane perf-A11). 7c303299..54a8bdf7 holds only A9's five commits.

| test | main 5e6c619f | before A9 7c303299 |
|---|---|---|
| w7 | 0 of 6 passed | 0 of 1 passed |
| w8 | 6 of 6 passed | 6 of 6 passed |
| w11 | 0 of 6 passed | 0 of 1 passed |

r1245's other runs (main's binary, `452 filtered out`): w7 0 of 5 passed, w11 1 of 5. So on main w7 is red in 11
of 11 runs and w11 in 10 of 11.

A failing run ends at its read's 120 s deadline (`get(..).unwrap()` on `Err(Timeout)`, sql.rs:2952). In w7 the
modes TwoPhase/Decided/Committed, TwoPhase/Resolved/Committed and TwoPhase/Decided/Wait pass; the next,
TwoPhase/Decided/WaitOnLock, never answers after the gate opens, on both commits; w11 fails on its WaitOnLock
mode too (sql.rs:3281, `left: Err(Timeout)`, `right: Ok(Some([8]))`). So w7 and w11 failed before A9
landed, in the same mode with the same timeout: they are not A9's (A9 touched no xgroup, replica or read path), and are left for the lane that owns CP-T1.10's locked reads. A guess at where to look,
not checked: 968b9c1a (11-22, "a read across groups held by a lock wakes only for its own row's resolution")
narrowed the wake that 1495f4fb's WaitOnLock replica reads depend on.

What is left for lane A11 (decisions 3a, 3b and unit 4; not started here, so no code on perf/cross-a10):

- 3a. `Operation::Call(Mutex<Option<Call>>)` gains its declared bound (`Operation::Call(call, answer_bytes)`);
  `Admission::bytes_for` charges it, so `Admission::reserve_call` and the `answer: Option<usize>` threaded
  through `session`, `call_db_session`, `submit_selected` and `submit_local` go. `call_internal` and `call_db`
  take an `AnswerMax<R>` from each caller (xgroup/sql.rs 6 sites, database/across.rs 5, restore.rs 4,
  online.rs 2, transport/common.rs 2, postgres/listener.rs 1, application.rs 1). In
  database/replicated.rs, `Handle::submit` enqueues every Submit at `api::REQUEST_MAX` (line ~1538): charge
  `bytes_for(&operation)` instead; the test sites (~10, each `enqueue(Input::Submit{..}, REQUEST_MAX)`) follow.
  `queue_bounds` passes `REQUEST_MAX` as a call's charge to `command_counts` (the most calls the room holds),
  and it and `owner_image_room` hold one call's `REQUEST_MAX` of room back beside the wire record; the test at
  replicated.rs ~5709 asserts that count (`application / (REQUEST_MAX + ENVELOPE_BYTES)`). Those are bounds
  derived from the largest call, not per-call charges, and need their own decision once no call is charged it.
- 3b. serve.rs's write path calls `db.call(until, AnswerMax::bytes(RESULT_MAX), ..)`. Each frame is held by its
  own `Charged` as it's built and `Encoded::answer_bytes` counts only unheld bytes, but the answer's
  `Vec<Encoded>` spine is not charged, and its frame count has no bound before the call runs. Charging the
  spine as frames are pushed would let the call declare `size_of::<Vec<Encoded>>()` alone.
- 4. `store::Config::queue` (`Some(4096)` by default, `Groups` and tests/store.rs's `Some(1)`) and
  `ROOM_LOOK` (store.rs:516, the 5 ms re-look at :1452 and :1549) remain; bench/src/store.rs:195
  (`AMPHORA_QUEUE`, V4's bba65c73) sets the count and must go with it. The `Engine` trait has no row size yet.

**r1265, added by lane perf-A11.** From the copied 7c303299 lib test binary (every log says `449 filtered out`,
that commit's count), w7 and w11 each alone in its own process, runs 2-6, both tests at once (A10's
`cross-a10/pre2.sh`; logs `~/.cache/amphora-perf/a10-pre2-7c303299-*` on adacachy2, 09:26-09:34 PST):

| test | passed | failed (read's 120 s deadline) | cut off |
|---|---|---|---|
| w7 | 0 | 4 (runs 2-5; sql.rs:2952 then 3005) | 1 (run 6: only `running 1 test`, started 09:34:46 as the run ended) |
| w11 | 1 (run 3, 0.42 s) | 4 (runs 2, 4, 5, 6; sql.rs:3281, `Err(Timeout)` vs `Ok(Some([8]))`) | 0 |

With r1245's one run each, before A9 w7 failed 5 of 5 complete runs and w11 5 of 6, in the same places as on
main (w7 11 of 11, w11 10 of 11). A9 did not make them fail.

## A call's charge carried in Operation::Call; the replicated owner admits calls by their bytes (perf-A11)

The coordinator's decisions on A10's questions: a replicated owner's queue admits by the declared sizes of the
calls actually waiting, in bytes, not a count and not the largest bound seen; no fixed reserve for replication
messages (votes, heartbeats and their answers get their own path, lane P9; log appends charged their real
size like calls).

**Unit 3a, part 1 (f2ff9b58, 4c529b66).** `Operation::Call(Mutex<Option<Call>>, usize)` carries the bytes its
caller declared its answer holds, so `Admission::bytes_for` charges a call that. `Admission::reserve_call` and the
`answer: Option<usize>` threaded through `session`, `call_db_session`, `submit_selected` and `submit_local` are
gone. A replicated owner's `submit` charges `Admission::bytes_for(&operation)` (a Records operation its real
size, a call its declared bytes), not `REQUEST_MAX` a request. The owner's queue no longer counts calls apart:
its call count was what the intake took in an election timeout at `REQUEST_MAX` a call (`command_counts`), so
with calls charged their declared bytes the count, not the bytes, would have bound them. Calls are now held by
the bytes those waiting hold (`called` against `applications`) and take any of the queue's inputs but the
last, which stays a wire record's. Codemods: `~/.cache/amphora-perf/cross-a11-scripts/*.py`.

Tests: `a11_replicated_queue_admits_waiting_calls_by_their_declared_bytes` (two calls declaring a third of the
calls' room admitted, the third refused `Full` at the calls' bytes through `submit`, typed; one more small
declared call than the room held at `REQUEST_MAX` each, all admitted);
`unknown_leader_application_pressure_keeps_wire_admission_available` rewritten to bytes (calls declaring an
eighth of the room: 7 admitted, the 8th refused `Full` at the calls' bytes, a heartbeat still admitted and
processed); `t_qb_1_6` counts inputs only; `mb1_*` asserts a call is charged its declared bytes.

Still at `REQUEST_MAX`, so not done (the rest of 3a):

- `call_db`/`call_internal`'s 19 callers (xgroup/sql.rs 6, across.rs 5, restore.rs 4, online.rs 2,
  application.rs 1, call_internal itself) and `Database::query` on a replicated database. Their answers
  (`Reply` and its dropped locks, `Held<T>`, `Stepped`, `ImportProgress`, `Option<Rev>` lists) have no
  `AnswerBytes` impl, and a declared bound without the `within` check would be a charge nothing holds the
  answer to. Next: `AnswerBytes` for those types, then `AnswerMax<R>` on `call_db`/`call_internal` (and on
  `query`, ~280 lines name it), each bound derived from what the closure reads.
- The wire record's byte room held back from calls (`queue_bounds`: `applications = queued - wire - envelope`;
  `owner_image_room`: `room - REQUEST_MAX`; the record's `min(room - REQUEST_MAX)`) and the queue's last
  input kept for a wire record. The decision removes them once votes and heartbeats have P9's own path; P9 is
  not on main at 4c529b66, and removing them first would let waiting calls refuse the heartbeat that finds a
  leader (the pressure test above). Log appends (`Input::Receive`) are already charged their bytes.

What is left for the next cross lane (3a's rest above; 3b and 4 not started here):

- 3b, on branch perf/cross-a11 (3c22e6ae, 46f03fef), NOT merged: serve.rs's frame list is `Frames`,
  its slots charged to the budget before the list grows (`Frames::push`, a typed refusal past the room), each
  frame holding its own bytes as before; its `AnswerBytes` is its own size plus any unheld frame's bytes, so the
  write declares `Frames::DECLARED` (`size_of::<Frames>()`) in place of `RESULT_MAX`. Refusal and agreement
  answers are one uncharged frame (`Frames::one`). Test: `cp_t1_13_a_sql_write_answer_past_one_frame_*` declares
  `Frames::DECLARED` and commits a write answered in many frames. Not tested: a refusal of the list's own growth
  for memory (needs a budget held short mid-answer). r1350 at 46f03fef (adacachy2, needs cpus=4,mem=16g, load
  26-33): clippy -p amphora --all-targets clean; lib `serve::` 11/11, `database::admission_charge` 2/2,
  call_nowait_charge 1/1; tests/serve.rs 9/10, `cp_t1_13_sql_takes_bound_parameters_and_answers_typed_rows_with_their_names`
  failed at tests/serve.rs:759, the `unwrap` of a 20-statement write carrying 20 MiB of blobs, whose answer is
  one small frame. Its error text was not captured (the check greps it out) and the re-run alone was refused
  (adacachy2 at 1 of 32 cores idle), so whether 3b or the host's memory refused it is not known. Next: run that
  test alone at 46f03fef and at main 66484d49 and read its `Err`.
- 4. `store::Config::queue`, `ROOM_LOOK` and its re-looks, and bench/src/store.rs's `AMPHORA_QUEUE` (bba65c73)
  are as A10 found them.


## 3b's red serve test is main's own; 3b merged (perf-A12)

**Unit 1.** r1374 (adacachy2, needs cpus=4,mem=16g, load 16-18 on 32 cores, 81-83 GB available): the
`tests/serve.rs` binary built at 46f03fef (3b) and at 66484d49 (main before 3b), then
`cp_t1_13_sql_takes_bound_parameters_and_answers_typed_rows_with_their_names --exact --test-threads=1` alone,
three runs each. Script: `~/.cache/amphora-perf/cross-a12-scripts/u1.sh`.

| commit | failed | error at tests/serve.rs:759 |
|---|---|---|
| 46f03fef (3b) | 3 of 3 (1.8-2.1 s) | `Err`: "the commit outcome is unknown; reconcile it before retrying the operation" |
| 66484d49 (main) | 3 of 3 (1.8-2.5 s) | the same `Err` |

The 20 MiB write's `unwrap` meets `Refusal::UnknownOutcome` on both, never a memory refusal (`Full`/`TooLarge`):
the long write loses its leader before it answers (lanes P6-P10), not 3b's frame list. 3b merged as it was
tested in r1350 (clippy clean, lib `serve::` 11/11, `database::admission_charge` 2/2, call_nowait_charge 1/1,
tests/serve.rs 9/10 with this test the one red); main 66484d49..993d9346 changed no Rust, so the merge
re-tests nothing.

A second test went red once beside it: r1395 (clippy and the touched tests at the merge 5c28ad0a, load 13-20)
had tests/serve.rs 8 of 9 with `t_t1_13_1_a_region_lost_is_replaced_by_a_spare` failing at :206 (`member add 4`:
"no voter made the change: NotLeader { leader: Some(3) }", the cut member still the leader heard). Alone from
the two binaries (r1403 3 runs each, r1415 12 each, load 13-60): 5c28ad0a 14 of 15 passed, main 57b584ee 14 of
15 (its failure `member promote 4`: UnknownOutcome). The same membership race on both, so not 3b's.
In the suite (`--test-threads=4`, cp_t1_13 skipped): red 2 of 2 right after a `cargo test` build (r1395,
r1422), green 5 of 5 from the built binary at 6d10f9e5 (3b + 2a) and 5 of 5 at main d5e901f3 (r1428, load 27-31).
3b with 2a at 6d10f9e5 (r1422): clippy -p amphora --all-targets clean; lib `serve::` 11/11,
`database::admission_charge` 3/3, `database::online` 6/6; call_nowait_charge 1/1.

**Unit 2a (f0fff576).** The answers of `call_db` and `call_internal`'s callers count their bytes: `AnswerBytes`
for `Result<T, E>`, `Refusal` (its strings' and key's capacity), `api::Row`, `ImportProgress`, across.rs's
`Held<T>`, online.rs's `Stepped` (also `FixedAnswer`: a `u64` and a `Duration`), xgroup's `Reply`, the
coordinator's `Answer`, `Application` and `FloorRefused`; `heap_bytes` gives a field's heap for a type that holds
it. Nothing charges them yet: the callers still pass `REQUEST_MAX` (2b). Test:
`database::admission_charge::a12_call_db_answers_count_their_heap`. r1414 at f0fff576 (adacachy2, needs cpus=4,mem=16g, load 35-54): clippy -p amphora --all-targets clean; lib
`database::admission_charge` 3/3, `database::online` 6/6.

**Unit 2b, not built (what the next lane does).** `call_db(until, answer: AnswerMax<R>, f)` and
`call_internal(until, answer, f)` with `R: AnswerBytes`, the closure wrapped in `within(f(db)?, answer)` as
`call_session` is, and `answer.get()` to `call_db_session`. Each caller's bound, derived from what its closure
reads (these are the derivations; none is picked):

- `AnswerMax::fixed()`: xgroup `prepare` and the tests' `put_lock`/`drop_locks_of`/`set_record` (`()`);
  across.rs's stamp wait (`()`); online.rs's `step` (`Stepped`) and `abandon` (`()`); restore.rs's
  `apply_page` (`()`); the across tests' `Option<Option<Rev>>` and `Option<u64>`. The across test's
  `(Vec<Rev>, Vec<Rev>)` takes `RESULT_MAX`, as A9's test row lists do.
- restore.rs `progress`/`restore_progress`: `size_of::<ImportProgress>()` (in an `Option` for the latter) + the
  source's bound (restore.rs:58, 256) + `CURSOR_MAX`. `finish`: check its answer type (`()` expected).
- application.rs `raise`: `Result<Application, FloorRefused>`: `size_of` + `FAMILY_MAX` (the name's grammar
  bound, `Application::validate`) or the `FloorRefused` refusal's static text.
- xgroup `step(group, carried, until, answer, f)`: `serve`/`step_on` declare `(Reply, Vec<Vec<u8>>)` from the
  envelope before it moves into the closure: Begin/Decide*/Recover*/Prevent*/Resolve* the inline reply plus a
  `Staging` list of at most `TX_MAX` `GroupId`s (participants are at most the transaction's keys,
  `coordinator::validate`); Scan an `Open` page of `SCAN_MAX` `(TxnId, GroupId)`; Lock/LockIf `reads.len()`
  entries of `key.len() + VALUE_MAX` or one key (`Conflict`/`Unmet`, `KEY_MAX`). The dropped list: at most the
  transaction's `TX_MAX` keys of `KEY_MAX`. `commit`'s `one_group` (`Answer`): `txn.reads` with `VALUE_MAX`
  each, or one key. Open question for that lane: a `Vec`'s spare capacity is counted by `AnswerBytes`, so a
  spine grown by `push` needs its growth in the bound (or a shrunk spine before answering).
- across.rs `waiting(group, until, answer, read)`: callers declare `Result<Held<T>, Refusal>`: a page
  `RESULT_MAX` (the page bound, lane perf-A6), a row `size_of::<Row>() + KEY_MAX + VALUE_MAX`, a stamp
  fixed; `Held::Locked` a key of `KEY_MAX`; the refusal `Compacted` (inline) or `unstamped`'s one format
  string with one `u64` (its literal length + 20 digits).

## 2b built and merged; units 3 and 4 not reached (perf-A13)

**Unit 2b (af9692f3, b72554f5; with main 9c0ece85 as bf9c2e43).** A12's WIP (60e63f8b) on main cfa0107c, whose
K11 `call_internal_ringing` (a step's answer and its dropped keys, rung once committed) still submitted
`Operation::Call(.., REQUEST_MAX)`: it now takes the step's `AnswerMax<(R, Vec<Vec<u8>>)>`, charged at admission
and checked inside the call by `within`. `AnswerBytes` for `api::Page` and `api::Change`. across.rs
`waiting(group, until, answer, read)`, each read's bound from `held_max(heap)`: the answer's heap (a scan page
`page_bytes::<Row>(scan.limit)`, by `fits`; changes `page_bytes::<Change>(limit)`, by `whole`; a get its key and
`VALUE_MAX`; `changed` none), or the locking row's key (`KEY_MAX`: every transaction key passes `api::key` in
`coordinator::validate`), or the refusal (`Compacted` inline, or `unstamped`'s text at `Rev::MAX`), whichever
is most.

Coordinator's decision 2 (lists built a push at a time): `grown` (twice the entries) is gone; each answer list
is trimmed before it answers and the bounds count entries exactly: `page_at`'s rows, `changes_in`'s changes
(collected in place over the wider `(stamp, seq, Change)` list, so their capacity was the wider list's),
`apply_step`'s reply lists (`Reply::trim`) and dropped keys, `one_group`'s reads (`trim_reads`), and
`unstamped`'s text (`format!` reserves twice its literal pieces). The trim's cost is not measured (coordinator,
11-23: after the deliverable works, rule 0).

Coordinator's decision 1 (wide lock steps): not paged. A lock step can't be split per group without changing
recovery: `Prevent` asks each participant whether it holds the transaction's locks and "all present" commits
it implicitly (docs/design/xgroup.md:33-37, Parallel Commits), so a group holding one page of a split lock step
would count as present with keys never locked. The lock step declares its real worst case instead: each key it
reads with a `VALUE_MAX` value (`reads_bytes`), or the one key a conflict or condition names.

Tests: `xgroup::sql::answers::a13_lock_reply_at_its_worst_case_holds_what_its_step_declares`,
`database::across::tests::a13_unstamped_refusal_is_within_every_read_bound`,
`database::across::tests::a13_page_at_its_worst_case_holds_what_a_scan_declares`. Script:
`~/.cache/amphora-perf/cross-a13-scripts/check.sh` (adacachy2, needs linux/amd64,cpus=8,mem=24g):

| run | commit | result |
|---|---|---|
| r1473 | af9692f3 | clippy: 2 `arithmetic_side_effects` in A12's restore.rs bounds (fixed in b72554f5) |
| r1480 | b72554f5 | clippy -p amphora --all-targets clean; lib `a13_` 3/3, `database::across` 13/13 (1 ignored), `database::admission_charge` 3/3, `database::online` 6/6, `xgroup::` 34/34; load 20-31 on 32 cores |
| r1485 | bf9c2e43 | the same, after merging main 9c0ece85 (V8): clippy clean; `a13_` 3/3, `database::across` 13/13 (1 ignored), `database::admission_charge` 3/3, `database::online` 6/6, `xgroup::` 34/34; load 26-34 |

Not reached (next lane): unit 3 (`Database::query` takes a declared bound) and unit 4 (resharding charges by
row; deletes `store::Config::queue`, `ROOM_LOOK`, bench `AMPHORA_QUEUE`).

## Unit 3 built, not merged; the group's own scan path trims (perf-A14)

**Before unit 3.** r1511 at main 5f511d8d (cb9717be plus S6's bench record; adacachy2, needs
linux/amd64,cpus=8,mem=24g, load 19-37): clippy -p amphora --all-targets clean; lib `a13_` 3/3,
`database::across` 13/13 (1 ignored), `database::admission_charge` 3/3, `database::online` 6/6, `xgroup::`
34/34. A13's merged tree holds; nothing to fix.

**2b check: the group's own scan path did not trim.** database.rs's `Operation::Scan` grew its page by
`push` and answered it as grown, while admission charges it `page_bytes::<Row>(limit)`. At the worst case
(`limit` rows at the largest key and value) a pushed spine holds the next power of two: 5 rows, 8 slots, 3
`Row`s past the charge. It is trimmed before it answers (254fc9d1). Test:
`database::admission_charge::a14_a_group_scan_at_its_worst_case_holds_what_admission_charged`.

**Unit 3 (254fc9d1; doc 82c3e84e).** `Database::query(until, answer: AnswerMax<R>, f)`, `R: AnswerBytes`:
charged `answer` at admission where it runs as a call (a remote or replicated database, before a flat
`REQUEST_MAX`), and checked by `within` wherever it runs. `query_session` and `query_waiting` take the bound
for reads at the leader's latest commit; `query_local`, `query_closed` and `query_local_waiting` read on this
node's state and take none (`query_reader`). A waiting run declares `met_max`: its answer, or the met row's
cross-group key, trimmed, at most `KEY_MAX`. Callers: `AnswerMax::fixed()` for counts and heads (tests,
guards.rs, xgroup's `head`, bench scale.rs and cluster.rs); xgroup's W11 reads an option plus `VALUE_MAX` (one
Records get's value). Test: `database::admission_charge::a14_a_query_at_its_worst_case_answers_within_its_declared_bound`
(3 blobs of 100 bytes, `LIMIT 3`: the answer holds exactly its derived bound; a byte less is refused
`TooLarge`). Codemods: `~/.cache/amphora-perf/cross-a14-scripts/*.py`.

| run | commit | result |
|---|---|---|
| r1550 | 254fc9d1 | clippy -p amphora --all-targets clean; lib `a14_` 2/2, `database::admission_charge` 5/5, `database::across` 13/13 (1 ignored), `database::online` 6/6, `database::guards` 13/13; `xgroup::` 33/34: `xgroup::lan::tests::ac_t1_10_5_seeded_histories_over_two_endpoints_show_no_anomaly` panicked at lan.rs:757 (`elle(..).unwrap()`, its `Err` not captured; lan.rs calls no query and no scan); bench not checked (`-p amphora-bench` matches no package: bench is outside the workspace); load 10-17 |
| r1557 | 254fc9d1 and 5f511d8d lib binaries | the lan test alone: 3/3 pass on each binary (0.42-0.48 s a run), so r1550's failure did not reproduce outside the suite; bench clippy `--locked` refused: bench has no committed Cargo.lock |
| r1559 | a6dab53e | clippy `--manifest-path bench/Cargo.toml --all-targets` without `--locked`, offline. Pending at hand-off |

Not merged: the bench crate, whose `query_session` callers changed, is unchecked. Next lane: read r1559;
with bench clippy clean, merge origin/main, re-run clippy and the touched tests, push HEAD:main. Unit 4 is
not started.

Seen on the way, not changed:
- A local query with no free reader runs on the writer through `Store::run`, the store's own bounded channel (`Config::queue`, unit 4), charged nothing at admission.
- `query_local`, `query_closed` and a session read served on a replica run on the caller's thread, charged nothing.
- `query_local_waiting` now needs `R: AnswerBytes` (it shares `session_waiting`), though its read takes no bound.
- A11's "~280 lines name `query`": the query family has 10 callers outside database.rs (5 `query`, 1 `query_waiting`, 1 `query_local_waiting`, 3 `query_session`); most `.query(` matches are `Db::query` and the engine's.
- across.rs's `Held::Locked` keys come from `xgroup::sql::locked`, built by `record_key` with `extend`, so capacity can pass length; A13 declared `KEY_MAX` for them; whether they're trimmed is not checked.

## Unit 3 checked on the merged tree; unit 4 not built (perf-A15)

**Unit 3's merge.** r1559 (A14's bench clippy, no `--locked`) ended rc 101; its list was cut at 40 lines by
`head -40`, so it couldn't show a compile error further down. The coordinator's bar for bench (11-23): no new
finding on the lines A14 changed, since bench's clippy fails on main itself (S7, T11, R1: 160-166 findings).
Script `~/.cache/amphora-perf/cross-a15-scripts/check15.sh` (adacachy2, needs linux/amd64,cpus=8,mem=24g):

| run | commit | result |
|---|---|---|
| r1582 | 4a2954bd (a6b275c4 + main 7cfe907e) | clippy -p amphora --all-targets clean; lib `a14_` 2/2, `database::admission_charge` 5/5, `database::across` 13/13 (1 ignored), `database::online` 6/6, `database::guards` 13/13, `xgroup::` 34/34; bench clippy 164 findings (main's own: println, env::args), 0 compile errors, none on A14's lines (cluster.rs 426-432, scale.rs 2014-2022); load 24-31 |
| r1601 | 9dc7a735 (4a2954bd + main 8feb1cbe: pg-parser, one engine optimizer file, bench planner, scripts) | the same results: clippy clean; `a14_` 2/2, `database::admission_charge` 5/5, `database::across` 13/13 (1 ignored), `database::online` 6/6, `database::guards` 13/13, `xgroup::` 34/34; bench 164 findings, 0 compile errors, none on A14's lines; load 18-32 (adacachy2 refused its first submission at 1 of 32 cores idle) |

NOT on main. Main moved after r1582, so the merged tree was checked again (r1601, green). Main then moved three more times before the push: records and scripts/engine-ratchet.toml; bench/src/store.rs and suite_db.rs (lanes W1, W2); then crates/amphora/src/database/replicated.rs. The last merge, e0fcbc82 on perf/cross-a13, is not checked: adacachy2 refused its check twice at 22:03Z (2 of 32 cores idle). Next lane: run check15.sh at e0fcbc82 (or a fresh merge) with the same filters, then push HEAD:main.

**Unit 4, read and not started.** What the next lane needs:
- `Config::queue` users: `Config::default()` is `Some(4096)`, so `Groups` (groups.rs:467, :525 take the caller's
  config), reshard/replicated.rs tests (:424, :522), database.rs tests (:5180, :5298, :5319), tests/crash.rs:628,
  tests/store.rs (`Some(1)`), bench/src/store.rs:195 (`AMPHORA_QUEUE`) and bench/src/suite_db.rs:317 (`..defaults`).
  A `Database`'s own store already passes `None` (database.rs:2120).
- `ROOM_LOOK` (store.rs:516) is reached only on `TrySendError::Full`, in `hand` (:1452) and `run` (:1549): only a
  counted queue reports `Full`. Why it polls: the writer calls `room.1.notify_all()` (:1727 for rows, :1741 for a
  run) without taking `room.0`, and a sender's `try_send` and its wait are not under that lock, so a wakeup
  between them is lost. An event-driven wait keeps the room's state (bytes queued) under `room.0`: the sender
  charges and checks under the lock and waits on the condvar until its deadline only; the writer releases a
  batch's bytes under the lock, then notifies.
- "By row" needs a row's size, which the `Engine` trait doesn't have (A11's note still holds): `E::Row` has no
  byte size today. A `Database`'s row is a `Command`, whose charge is `Admission::bytes_for(&operation)`.
- A standalone `query` with no free reader: `Store::read`/`read_committed` fall back to `self.run(until,
  on_writer)` when `readers.with` gives `None` (store.rs:1484, :1521), queuing a `Msg::Run` on the unbounded
  queue with no charge; `Database::query_reader`'s writer fallback is the place to charge it at admission (its
  `AnswerMax` plus the closure).

## Unit 3's landing re-checked; unit 4 built, unchecked (perf-A16)

**Unit 3.** r1629 (adacachy2, needs linux/amd64,cpus=4,mem=12g, `~/.cache/amphora-perf/cross-a16-scripts/check16.sh`: check15.sh held to 4 jobs and 4 test threads) at c79b2d53 (e0fcbc82 + main 98bd0fa9: pg-parser tests): clippy -p amphora --all-targets clean; lib `a14_` 2/2, `database::admission_charge` 5/5, `database::across` 13/13 (1 ignored), `database::online` 6/6, `database::guards` 13/13, `xgroup::` 34/34; bench clippy 164 findings (main's own), 0 compile errors, none on A14's lines; load 42-49. Clippy's `Finished` took 8.67 s after replicated.rs changed: not explained. Main then moved inside `crates/` (engine sums and morsel scans, store.rs and bench suite_db.rs by lanes R4 and W3), so the landing merge b5738b12 (c79b2d53 + main 4b2a9793, branch perf/cross-a16-land) is checked again in r1680's first half. NOT on main.

**Unit 4 (5e5ae655 on perf/cross-a13, unchecked).** r1680's second half: clippy, lib `store::`, `database::`, `xgroup::`, and `--test store`.
- `Config::queue` and bench's `AMPHORA_QUEUE` deleted; the writer's channel counts nothing. A batch is charged `Engine::row_bytes` (new, required: each engine's inline size plus owned capacity) to the memory budget's room for queues until its commit ends, refused `Full` past it, counted.
- The room's state (this store's charged bytes) is under `room.0`: a sender holds and checks under the lock and waits on the condvar only until its deadline; `Queued`'s drop gives a batch's bytes back under the lock, then notifies. `ROOM_LOOK` and the two lock-free `notify_all` at take-off are deleted.
- A sender refused while nothing of its store holds a charge is refused at once: none of its commits could wake the wait, and the budget's other holders (other stores, admissions, caches, resident bytes) signal nothing.
- `Local::row_bytes` is 0: a Database command is charged at admission until answered.
- `query_reader`'s writer fallback is charged at admission before it is queued (`Store::on_reader` splits the reader attempt from `read_committed`): its job, its session and `query`'s declared answer. `query_local` and `query_closed` declare no answer, so their fallback charges only the job and session.
- Test `store::tests::mb1_a_wait_for_room_is_woken_by_the_commit_that_gives_room_back` (room for one row; deadline an hour off; `Counters::room_waits`, new, as its non-vacuity count) replaces tests/store.rs's counted-queue test.
- Main 4b2a9793 conflicts with 5e5ae655 in crates/amphora/src/store.rs (W3's flusher change): the next lane merges it.

## Unit 3 landed; unit 4 checked green, not on main (perf-A17)

**Unit 3 on main (626b0a90).** While unit 3 waited, main's V11 (ad4c0922, CP-T1.8) changed `Database::query`: a replicated query runs past its group's barrier as `Operation::Query`, charged a flat `REQUEST_MAX`. The merge 96b83947 keeps both. The replicated arm submits `Operation::Query` charged `answer.get()`, and `within` checks the answer inside the call, as `call_nowait` does. The attached client's session takes `answer.get()` and `within`. Script `~/.cache/amphora-perf/cross-a17-scripts/check17.sh` (check16.sh with `JOBS`, `STORE` and `BENCH`; adacachy2):

| run | commit | result |
|---|---|---|
| r1712 | 96b83947 (b5738b12 + main dd9c3ce7) | needs cpus=4: clippy -p amphora --all-targets clean; lib `a14_` 2/2, `database::admission_charge` 5/5, `database::across` 13/13 (1 ignored), `database::online` 6/6, `database::guards` 13/13, `xgroup::` 34/34, and V11's `a_follower_read_completes_while_another_member_holds_the_write_slot`, `a_replicated_query_that_writes_is_refused_and_writes_nothing`, `a_read_barrier_asks_for_no_slot_and_ends_its_members_own` 1/1 each; load 34-48 |
| r1732, first half | 848a16c2 (96b83947 + main c75bb13f: engine types, vdbe, functions) | needs cpus=8: the same set, all green; load 28-33 |

Pushed as 626b0a90: 848a16c2 + main 8c426f7b, which moved only crates/pg-parser and docs/requirements.toml. No crate in the workspace or bench depends on amphora-pg-parser, so no new run. A16's r1684 (macOS, b5738b12, never started) was stopped: r1712 had checked the newer merge.

**Unit 4, NOT on main.** 5e5ae655 merged with unit 3's landing (b7910c4a, cf13e776). store.rs conflicted with W3's flusher change (81fc0dca, via 6a3e8ebb) at the writer's take-off of `Msg::Rows` and `Msg::Run`: W3 deleted `Sends::went_busy`, unit 4 the lock-free `room.1.notify_all`. Both deletions are kept. Unit 4 compiled as A16 wrote it.

| run | commit | result |
|---|---|---|
| r1732, second half | cf13e776 | needs cpus=8: clippy -p amphora --all-targets clean; lib `store::` 5/5 (`mb1_` among them), `database::` 253/254 (6 ignored), `xgroup::` 34/34; tests/store.rs 14/14; bench clippy 164 findings (main's own count), 0 compile errors, none on unit 4's `row_bytes` lines; load 33-48 |
| foreground, no id | 848a16c2 and cf13e776 lib binaries | `database::`'s failure alone, `t_t1_10_r1_a_session_reads_its_write_on_a_lagging_follower`: 10/10 pass on each binary; load 31-36 |
| r1756 | the same binaries, `database::` at 8 threads | round 1: 848a16c2 (no unit 4) fails the same test (253/254), cf13e776 passes 254/254; load 23-51 |

The failure is main's own: session_reads.rs:122 asserts `counters.waited == 1` once `behind` was read before the reading thread starts, so under the suite's load the follower can catch up before the read waits. It fails on the arm without unit 4.

Main moved inside `crates/` twice more before the push, so each merge was checked again with the same set:

| run | commit | result |
|---|---|---|
| r1762 | a36e1cce (22ccfa30 + main 41422da9: R5's engine sums and column scans) | needs cpus=8: clippy clean; lib `store::` 5/5, `database::` 254/254 (6 ignored), `xgroup::` 34/34; tests/store.rs 14/14; bench clippy 164 findings, 0 compile errors, none on unit 4's lines; load 21-34 |
| r1776 | 3c9425d3 (a36e1cce + main ef34f511: W4's store.rs, decision 9, and its crash test) | needs cpus=8: clippy clean; lib `store::` 5/5, `database::` 254/254 (6 ignored), `xgroup::` 34/34; tests/store.rs 14/14; tests/crash.rs 10/10 (3 ignored); bench clippy 164 findings, 0 compile errors, none on unit 4's lines; load 19-37 |

3c9425d3 conflicted in store.rs's `Sends`: W4 replaced `claimed: Mutex<u64>` with `claims: Mutex<Claims>`, unit 4 made `room` a `(Mutex<u64>, Condvar)`. Both kept.

Seen on the way, not changed:
- `Budget::hold` refuses `Unavailable` where the memory limit can't be read. `Store::charge` treats every refusal as no room, so on such a platform a batch with a nonzero `row_bytes` is refused `Full` at once, as `Admission::charge` already refuses a database's requests there.
- r1740 (the first try of the 10/10 runs) ended and was collected before its output was read: the kiln server did not answer in time.
- Unit 4's push of 99b67668 (3c9425d3 + records-only main) was rejected: main had moved inside `crates/` (B15's engine WAL, d0660c11). Its merge ff71d6f8 (on perf/cross-a13) is unchecked. adacachy2 refused its check at cpus=8 (r1781) and at cpus=4 (r1784): 0 of 32 cores idle, 23:54Z. Next lane: run check17.sh at ff71d6f8 (or a fresh merge) with `JOBS=4 STORE=1 BENCH=0` and `store:: database:: xgroup::`; if green and main moved only outside amphora's build, merge and push HEAD:main.

## Unit 4 and a wait for room woken by any charge given back (perf-A18)

**Where the checks ran.** macOS first: r1795 (unit 4's landing) and r1841 (the wait, then the red run) waited 26 and
20 minutes for a Mac with 8 idle cores (no Mac answered the balancer's probes; at 00:26Z the kiln server could not be
reached from adacachy) and were stopped before they started once r1892 ran the same checks. adacachy2 refused
cpus=8 (4 of 32 cores idle) and cpus=4 (0 of 32 idle) at 00:16Z, then admitted cpus=4 at 00:27Z. Script
`~/.cache/amphora-perf/cross-a18-scripts/check18.sh` (check17.sh with `CRASH=1` for tests/crash.rs and `CLIPPY=0`).

| run | commit | result |
|---|---|---|
| r1892, first part | 14e6d081 (unit 4: ff71d6f8 + main a34b83b1, then main 2f29b288: engine page cache, on-disk format, learned costs) | needs cpus=4: clippy -p amphora --all-targets clean; lib `store::` 5/5, `database::` 254/254 (6 ignored), `xgroup::` 33/34; tests/store.rs 14/14; load 54-113 |
| r1892, second part | 72d62e76 (the wait on 14e6d081) | clippy clean; `store::` 6/6, `database::` 254/254 (6 ignored), `xgroup::` 34/34, tests/store.rs 14/14, tests/crash.rs 10/10 (3 ignored); load 103-140 |
| r1892, third part | dbeb960a (14e6d081's unit 4 + the new test alone) | red: the new test fails in 0.06 s at store.rs:1849 (`waiter.join().unwrap().unwrap()`: store B's `put_wait` refused `Full` at once) |

r1892's one `xgroup::` failure on 14e6d081: `xgroup::lan::tests::t_t1_9_r18_the_lan_harness_runs_over_network_endpoints`
panicked at lan.rs:720 (`measure(..).unwrap()`; its error was not captured). The `xgroup::` run took 17.95 s against
the measurement's 120 s deadline (`transport::HANG`), so it was not the deadline. It passed on 72d62e76, which holds
all of 14e6d081. Not run alone: adacachy2 refused cpus=2 at 00:34Z (0 of 32 idle). Whether unit 4's refusal at once
refused one of the harness's writes, which the wait now makes wait, is not shown.

**The wait (bdfb19ff, f4e87ed0).** A sender whose store held no charge was refused `Full` at once, since only its own
store's commits woke the wait. Now the budget wakes its waiters:
- `memory::Budget::take_until` looks for room and, finding none, waits until its holder's deadline, woken only by
  charges given back anywhere in the budget (`Budget::give`: every `Charge`'s drop and the caches' give-back), never by a
  timer. A refusal other than no room (`Unavailable`) returns at once.
- A give with no holder waiting takes no lock. Holders waiting are counted. The waiter counts itself, then a SeqCst
  fence, then looks. The giver's give-back and its read of the count are SeqCst. So the look sees the bytes given back
  or the giver sees the waiter (C++ [atomics.order]'s single total order; Linux's `waitqueue_active`).
- store.rs's `Queued` and `Sends::room` are gone: a batch's charge is a `memory::Charge`, and `Store::charge` asks the
  budget to wait (`room_waits` counts each wait, under the budget's lock).
- The rule is in docs/design/store.md, "Decision (lane perf-A18): a wait for room is woken by any charge given back".
- Test: `store::tests::mb1_a_wait_for_room_is_woken_by_another_stores_charge_given_back` (store B, holding no charge,
  waits with a deadline an hour off; store A's commit gives its charge back; B's row is taken, one wait counted).

Seen on the way, not changed:
- A batch past the whole limit waits out its deadline: no give can make room for it. Resident bytes that fall wake no one.
- A give wakes every waiter, and each looks again.
- `Budget::give`'s give-back and read are SeqCst where they were AcqRel and absent: the same instructions on x86-64
  (a locked `cmpxchg`, a plain `mov`), an `ldar` for an `ldr` on aarch64. Not measured: the A/B runs after the merge (rule 5b).
- Requests admitted to a database (`Admission`, serve.rs's frames) still refuse at once; only a store's `put_wait` and
  `put_then` wait.

Main moved inside `crates/engine` twice more before the push, so each merge was checked again with the same set:

| run | commit | result |
|---|---|---|
| r1922 | ffe2fa35 (72d62e76 + main a48b29f2: R6's column copy choice) | needs cpus=4: clippy clean; `store::` 6/6, `database::` 254/254 (6 ignored), `xgroup::` 34/34, tests/store.rs 14/14, tests/crash.rs 10/10 (3 ignored); load 65-105 |
| r1940 | 473b207e (ffe2fa35 + this record + main 90c1ff8d: B16's checkpointer segment) | needs cpus=4: the same, all green; load 28-81 |
| r1944 | c1c0e531 (473b207e + these rows + main b1a92917: V13's database/replicated.rs and slot.rs) | needs cpus=4: clippy clean; `store::` 6/6, `database::` 256/256 (6 ignored), `xgroup::` 34/34, tests/store.rs 14/14, tests/crash.rs 10/10 (3 ignored); load 17-26 |

Unit 4 (14e6d081) and the wait (bdfb19ff, f4e87ed0) land together. Unit 4 alone was not pushed first: its own check had `xgroup::` 33/34, and the tree with the wait was green on every check.

## A19 checked and merged; the replicated ladder can't show it (perf-A20)

**Where the checks ran.** adacachy2, needs cpus=4, script `~/.cache/amphora-perf/cross-a19-scripts/check19.sh`
(A20's copies: `cross-a20-scripts/check20.sh`, which fetches perf/cross-a20, and `check20m.sh` for macOS, which runs in
kiln's synced copy of the lane clone and refuses a synced HEAD other than the commit named). Each lib-test filter below
holds A19's test of that module (`memory::tests::a19_…`, `database::admission_charge::a19_…`, `serve::tests::a19_…`).

| run | commit | result |
|---|---|---|
| r7, 01:59Z | 1a19ead8 (A19) | clippy -p amphora --all-targets clean; `memory::` 11/11, `store::` 6/6; then collected by kiln's restart on adacachy (~02:00Z) inside `database::` |
| r7 again, 02:02Z | 1a19ead8 | clippy clean; `memory::` 11/11, `store::` 6/6, `database::` 257/257 (6 ignored), `xgroup::` 34/34, `serve::` 12/12, tests/store.rs 14/14, tests/crash.rs 10/10 (3 ignored); load 30-49 |
| r27, r40 (macOS, cpus=8 and 4) | 2912576e (1a19ead8 + 502375c9 + main 8738fcb9) | queued from 02:05Z and 02:10Z behind other lanes, then collected by kiln's restart on adacachy (~02:18Z) without starting. adacachy2 refused cpus=4 and cpus=2 meanwhile (1 of 32 cores idle); one cpus=2 admission (r34) ran nothing, its script not yet written there (exit 127, A20's slip) |
| r10, 02:34Z | e7d493fc (2912576e + main 6d76681c) | needs cpus=4: clippy -p amphora --all-targets clean; `a19_` 3/3, `memory::` 11/11, `store::` 6/6, `database::` 260/260 (6 ignored), `xgroup::` 34/34, `serve::` 12/12, tests/store.rs 16/16, tests/crash.rs 10/10 (3 ignored); load 23-70 |
| r31, 02:39Z | 78a9fe56 (ecec204b + main c10b1a09: Q5's wal.rs and log_index.rs) | needs cpus=4: clippy clean; `a19_` 3/3, `memory::` 11/11, `store::` 6/6, `database::` 260/260 (6 ignored), `serve::` 12/12, tests/store.rs 16/16, tests/crash.rs 10/10 (3 ignored); `xgroup::` 33/34: `xgroup::lan::tests::ac_t1_10_2_the_lan_harness_measures_both_protocols_over_two_endpoints` panicked at lan.rs:736 (`measure(..).unwrap()`; its error not captured); load 45-71 |
| 02:44Z, foreground | 78a9fe56, r31's test binary | needs cpus=2: that test alone 5/5 (0.54 to 0.58 s each); `xgroup::` 36/36 (t_t1_10_w7 and t_t1_10_w11 included); load 53-59 |

The LAN harness's `measure` failing once under load is not A19's: A19 changes nothing in xgroup, the test passed on
e7d493fc (r10) and six times more on 78a9fe56's own binary, and A18's r1892 saw `t_t1_9_r18_…` fail the same way at
lan.rs:720 before A19 existed. Landed on clippy and the tests of what A19 touched (rule 5b); the error text is the
next thing to capture if it fails again.

| run | commit | result |
|---|---|---|
| r51, 02:45Z | 238114df (013846ba + main a5ae048c: B18's decision 45 switch) | needs cpus=4: clippy clean; `a19_` 3/3, `memory::` 11/11, `store::` 6/6, `serve::` 12/12, tests/store.rs 17/17; `database::` 257/260 (6 ignored), `xgroup::` 33/34, tests/crash.rs 9/10 (3 ignored); load 46-55 |
| 02:48Z, foreground | 238114df's binary, each failing test alone, twice | the three `database::replicated::tests` fail every time (below); `ac_t1_10_5_seeded_histories_over_two_endpoints_show_no_anomaly` passes 2/2 |
| 02:49Z, foreground | main a5ae048c alone (no A19) | the same three fail with the same messages, and tests/crash.rs's power cut fails: seed 45 of 200, "reopen: opening /crash/events.db: Corrupt database: the log's segments have checkpoint sequences Some(4) and Some(1), neither one past the other: no switch leaves such a pair" |

main a5ae048c's own failures, A19 absent (B18's decision 45 switch; they passed on 78a9fe56, main c10b1a09):
- `a_checkpoint_pass_copies_no_frame_past_the_last_finished_flush`, replicated.rs:16931: left 77, right 17.
- `a_local_read_open_across_an_image_install_sees_one_side_whole`, replicated.rs:16670: "the log restart under an open read: Ok(true)".
- `past_its_room_a_replica_refuses_submits_and_restarts_whatever_its_share`, replicated.rs:10557: Unavailable("the
  database's log holds 13 frames, past the 6 it may hold: an equal share of its filesystem's free bytes ...").
- tests/crash.rs `a_power_cut_at_any_change_loses_nothing_acknowledged_and_tears_no_batch`, crash.rs:821: a power cut at
  seed 45 reopens to a corrupt log (above). A durability defect on main, for lane B18's successor.

On 238114df every module A19 touches passes wherever main passes, and the tree's failures are main's own, the same
tests with the same messages (rule 5b).

main moved again inside crates/engine (T18's vdbe/execute.rs, 012b6b44), so the landing was checked once more:

| run | commit | result |
|---|---|---|
| 02:50Z | 9ef81742 (A19's landing: 238114df's records + main 012b6b44) | needs cpus=4: clippy clean; `a19_` 3/3, `memory::` 11/11, `store::` 6/6, `serve::` 12/12, tests/store.rs 17/17, tests/crash.rs 10/10 (3 ignored); `database::` 256/260 (6 ignored): the three above and `a_replicas_database_log_stays_within_its_room` (replicated.rs:16805); `xgroup::` not run (A19 doesn't touch it); load 28-31 |
| 02:54Z, foreground | main 012b6b44 alone, and 9ef81742's binary | `a_replicas_database_log_stays_within_its_room` fails 2/2 on main alone ("owner 1 after write 20: 125 frames, room 102, a write 6"); passes 2/2 alone on 9ef81742 |

So every failure on the landed tree fails on main without A19. The power cut passed 10/10 on 9ef81742 and failed at
seed 45 on a5ae048c: whether it is seed- or time-dependent is not known.

Landed on main as d8fcad27 (02:55Z): 9ef81742, these records, and main d22c749c, whose one change in crates/ since
012b6b44 is lane F1's whitespace-only `cargo fmt` of crates/engine/vdbe/execute.rs (the same text with whitespace removed).

**rustfmt (502375c9).** Of the five call sites A19 fixed in 1a19ead8, two ran past 100 columns (serve.rs's
`let SqlAnswer::Tables(tables) = answer(..)` at 2542 and 2583); rustfmt's two hunks are applied alone. At 2912576e `cargo
fmt -p amphora --check` also flagged main's own lines, present at 14fbf220 before A19 (database.rs, database/
replicated.rs, database/restore.rs, replication.rs, xgroup/sql.rs); lane F1's `cargo fmt` on main cleared them, and on
e7d493fc the check is clean.

**The merges (2912576e, e7d493fc).** main 8738fcb9, then 6d76681c, merged without conflicts. main's changes since A19's base touch none of what
A19 changed: no new caller of `Admission`, `read_frame`, `answer`, `Frame::Held`, `Shared::sql`, `take_until` or
`charge_queue`, and no new match on `store::Refused` (its one exhaustive match, database.rs 808
`admission_refusal`, is A19's). store.rs's change on main is the flusher's wait (decision 14), apart from `Store::charge`.

**The A/B through bench/repl_ladder.sh was not run: its writes never reach A19's code.**
- The ladder's members open `Database::open_replicated_with` (bench/src/repl.rs `procs::member_main`) and answer each
  write with `Database::call` (`procs::Member::request`, op `exec`). A replicated writer's call goes
  `Database::submit_selected` -> `Writer::Replicated { client }` -> `client.submit` (database.rs 3048): the replicated
  owner's `submit` sizes it with `Admission::bytes_for` and queues it with `enqueue` on its own atomics
  (database/replicated.rs 1507-1530), the path queued for a later lane. Only `Writer::Local` reaches `submit_local` ->
  `Admission::reserve` -> `Store::charge_queue` -> `Budget::take_until` (database.rs 3069, 3099, 107).
- `Budget::take_until`'s callers outside tests: store.rs 1393 and 1413 (`Store::charge`, a local store's batch),
  database.rs 107 (`Admission`, a local writer's), serve.rs 798 and 838 (a request's frame and its decoded charge).
  database/replicated.rs, database/slot.rs and replication.rs call none of them, nor `Store::put*`.
- e530666c to main 8738fcb9 is 77 commits. Besides A18's (bdfb19ff, f4e87ed0) they change the path the ladder times:
  a write slot's end (database/replicated.rs and slot.rs, V14's 7d08d4ff), the store's flusher (W5's 716ede4d and
  0e83fc98), a batch's charge (A16's unit 4, 5e5ae655), and engine/storage's log segments and wal.rs (Q3's 36ed445e,
  B17's 554b248e and 22173583). A difference between e530666c and main on the ladder could not be laid to A18 or A19.
- What does reach A19's code:
  - `amphora-bench --sql-api` (bench/src/sqlapi.rs: kiln's hot statements, each inside `Database::call` on a local
    `Database::open`, sqlapi.rs 146) -> `submit_local` -> `Admission::reserve` -> `take_until`, on every call: A19's
    admission and its lock-free first look. One client only.
  - The suite's `amphora` target writes through `Groups::put_wait` (bench/src/suite_db.rs 453) -> `Store::charge` ->
    `charge_queue` -> `take_until`, on every write, at any client count; not through `Database`.
  - The `amphora-serve` target reaches serve.rs's frame and request charges (`take_until`); its member's database is
    replicated (serve.rs 509), so not its admission.
  - No bench drives a local `Database` from more than one client (cluster.rs and scale.rs open replicated groups).
- On the way: `Store::charge` with no deadline (`Store::put`) now reads the clock once a batch
  (`until.unwrap_or_else(Instant::now)`), where it read none before. Not measured.

## A replicated write charged on the memory budget; not merged (perf-A21)

**Not merged.** The change works and its clippy and `scripts/check-production.sh` pass. Its last test fix
(f2cb8681) is unverified: at 03:52Z adacachy2 refused cpus=8 and cpus=4 (0 of 32 cores idle), and the Mac's
target dir was held by a main build (r140). Branch `perf/cross-a21` holds it all.

**The change (8a2c25bd, replicated.rs only).** `ReplicaHandle::enqueue` charges a write (`Input::Submit`) its
bytes (its operation's real or declared size, `Admission::bytes_for`, and its envelope) through
`Budget::take_until` on `Use::Queues` before it takes a place in the queue: the lock-free first try,
`TooLarge` at once past the whole limit, a wait woken only by bytes given back until the write's deadline, then
`Full`. The `Charge` lives in the write's `Reservation`, which already lasts from `enqueue` to the write's answer
(the channel, `waiting`, `paused_inputs`, `Pending`), so it is given back on every path: applied, refused, a term
change, an input dropped untaken, the owner's stop. `Control::charged` counts those bytes, and `rebound`'s
per-tick queues charge leaves them out (it charged `admitted` whole before), so no byte is charged twice.
QB.1's per-owner bounds (`commands`, `called <= applications`, `admitted <= queued`) still refuse at once after
the budget admits; `unknown_leader_application_pressure_keeps_wire_admission_available` asserts that refusal.

**Runs.**

| run | where, commit | result |
|---|---|---|
| r108 | MacBook Pro 2, needs cpus=8, 8a2c25bd, load 63-88 | clippy -p amphora --all-targets clean (8m59s); `a21_` 4/5 (term change: the write answered Ok); `database::` 260 passed, 5 failed (checkpoint-pass, image-install-read, past_its_room, the a21 term change, `v13_thirty_two_spread_writers…`); `memory::` 9/9, `store::` 6/6 |
| r138 | MacBook Pro 2, 8a2c25bd's test binary | `v13_thirty_two…` alone, 3 runs: 3 failed, `Persisted: row N at member 1: UnknownOutcome` (twice) and `Timeout` (once). main on macOS not yet run (r140 was building it) |
| r150 | adacachy2, needs cpus=8, d0f6a606 and main 7de5344d | clippy clean (37 s); check-production rc 0 (engine ratchet holds, 10903); `a21_` 4/5 (term change: `reach` timed out); suite r1 (`database::replicated:: replication::`): d0f6a606 172 passed, 6 failed (checkpoint-pass, the a21 term change, image-install-read, past_its_room, `session_reads::t_t1_10_r1…`, `every_falsifier_of_the_floor_holds_under_a_nemesis`); main 168 passed, 5 failed (image-install-read, past_its_room, `v3_two_replicated_writes…`, `a_local_read_holds_off…`, `a_restart_due_waits…`); load 52-59. `v13_thirty_two…` passed on d0f6a606. Stopped after round 1 to re-gate f2cb8681 |

| r180 | adacachy2, needs cpus=2 (cpus=8 and 4 refused), f2cb8681, load 50-61 | clippy clean (49 s); check-production rc 0 (ratchet 10903); `a21_` 4/5: the term-change test reached the write's proposal, the write unanswered, then expected `bytes_for`'s fixed bytes, which the driven `enqueue` helper doesn't charge (it charges `REQUEST_MAX` and the envelope). 7c753719 expects that; unverified. The run goes on to the suites (both arms) and the ladder, logs in `~/.cache/amphora-perf/cross-a21-out/` on adacachy2 |

**Why the term-change test failed three times.** At 8a2c25bd it dropped the leader's records one step after queueing
the write, before its barrier had sent any; at d0f6a606 it waited for the write's proposal, but its call wrote
nothing, so it passed a read barrier (`READ_BARRIER`) and never proposed. f2cb8681's write creates a table
through the driven `submit` helper, so it proposes, as 11873's change does.

**Open for the successor.**
1. 7c753719 (or the merge bb957f87's successor): `cargo clippy -p amphora --all-targets` and the five `a21_` tests, on the tree merged with main (main moved in crates/engine and crates/pg-catalog).
2. `v13_thirty_two…` on macOS against main 7de5344d (it passes on Linux with the change).
3. The replicated suites, rounds 2 and 3, both arms (`check21l.sh` in `cross-a21-scripts`), then V13's ladder
   at 32 and 128 writers, both classes (`ladder13.sh`), for zero refusals.

    # r150 (and its successor): kiln run on 00aad8daff2fa3c9daa65d53c425d450, needs linux/amd64,cpus=8,mem=24g,
    # sync false; ~/.cache/amphora-perf/cross-a21-scripts/check21l.sh BASE FIX ROUNDS LROUNDS OUT
    bash check21l.sh 7de5344d41dc850c022e829bdc285440c89c2095 f2cb8681fd80cfb95e82edfd83eb62e29485bc03 3 1 OUT

## A21's replicated-write charge, checked on the merged tree; v13's macOS failure is main's too (perf-A23)

**Lane times.** First command 04:58:43Z; the hour ends 05:58:43Z. Branch `perf/cross-a21`. The code in crates/ is
A21's (8a2c25bd, 7c753719); this lane's one code change is 63017ae8 (the term-change test, below). Merges of main:
0f65a665 (a64549cc: V17's close fix), 0ba7307d (3a58a13f: P20's and P22's serve landing, crates/amphora), a12bea22
(1f4a8e88: PG14's OID counter, crates/engine), cdcdef9f (731240ed: V18's close behind a follower's forward in
replicated.rs, V18's mvcc, F6's engine ratchet) and e53b671b (ac2c429a: T21's optimizer/lip.rs, crates/engine). `cargo
fmt --all --check` clean on each. main moved inside crates/ each time, so each was re-run on clippy and the touched
tests, at smaller `needs` when adacachy2 was busy; 8f59e5b8 (219603df: Q8's wal.rs and sqlite3_ondisk.rs, crates/engine)
was the landed tree.

**Losses.**
- v13 (`v13_thirty_two_spread_writers_on_moving_time_are_each_answered_with_no_stale_base`) fails on macOS on both
  sides, so the change is not what fails it. MacBook Pro 2 (r346, load 70-105), 3 runs a side, the order alternated:
  0f65a665 failed 3 of 3 (row 3 at member 1: UnknownOutcome; row 9 at member 1: Timeout, 142 s; row 15 at member 1:
  UnknownOutcome) and main a64549cc failed 3 of 3 (row 3 at member 1: UnknownOutcome, twice; row 9 at member 1:
  Timeout). Not traced here. On Linux it passed on both sides in every run (9 a side on the first merge and main
  a64549cc, 4 a side on 0ba7307d and main 3a58a13f's). Not run on macOS on main 3a58a13f, whose V13 test changed
  (P20's 8e0eced3).
- The brief's bar of three suite rounds a side with no failure belonging only to the change is NOT cleanly met. In the
  three planned rounds (r312) two tests failed only on the change's side: `a_restart_due_waits_…` (due) and
  `session_reads::t_t1_10_r5_…` (r5; its read counted `at_once: 1, waited: 0`). After seeing that, this lane ran three
  more rounds a side (r369): due then failed on main too (round 4); r5 failed on neither side again. Over all six
  rounds r5 is 1 of 6 on the change and 0 of 6 on main. Every other failure on the change's side also failed on main in
  some round. r5 alone afterwards: 0 failures of 70 a side (r365: 10, `session_reads::` by itself at load 29-30; r392: 40
  beside six busy loops, 20 without). That does not show the cause. The assertion would fit the test's read thread
  starting after the main thread had caught the follower up; that is inferred from the counters, not traced.
- check-production was last run on cdcdef9f (main 731240ed), not on the landed tree 8f59e5b8: since then main brought
  T21's optimizer/lip.rs and Q8's wal.rs and sqlite3_ondisk.rs. Clippy, the five `a21_` tests and v13 were re-run on it.
- The ladder was not run on main, and only on 0f65a665 and 0ba7307d, not on the landed tree. On 0ba7307d its commits/s is
  about half 0f65a665's, at a higher load (62-66 against 41-44) and with main's P20 and P22 between them: not compared, no
  cause claimed.
- The suites and the v13 counts on 0ba7307d and later trees are from one round and from v13 alone; the six-round table is
  0f65a665 and main a64549cc.
- The release bench and the ladder ran under load 41-66 of other lanes: it shows refusals, not throughput.
- Stopped by this lane: r316 and r331. The bench script had copied f2cb8681's stale binary after its own build failed on
  the untracked `bench/Cargo.lock` (fixed by building offline without --locked); one stale cell of r316 was seen, none is
  in this record.

**The term-change test and main 3a58a13f (63017ae8).** On 0ba7307d it failed, the write answered Ok though its term
moved. A proposal's append now goes out in the step that proposes it (`Actor::propose_stamped` sets `driving`, so the
owner steps again at once to send and write it; CP-T1.13, replication.md "Decision (lane P9)", part 3), so `handle.drain`
after the proposal no longer lost the append and the write committed. The appends are lost from the write's queueing
instead (`Pump::appends_lost`); the assertions are unchanged. On 63017ae8: `a21_` 5 of 5, 10 rounds (r432).

**Runs.** adacachy2 unless the row says macOS; each `kiln run` named its cores and memory in `needs` (refused ones
re-asked smaller). Scripts: `~/.cache/amphora-perf/cross-a23-scripts/` (`check23l.sh MERGE MAIN NV ROUNDS OUT`,
`bench23.sh MERGE LROUNDS OUT`, `check23m.sh` for macOS), outputs `~/.cache/amphora-perf/cross-a23-out/` there.

| run | what | result |
|---|---|---|
| r312, 05:03Z | 0f65a665 and main a64549cc, needs cpus=8 | clippy -p amphora --all-targets clean (31.7 s); check-production rc 0 (engine ratchet 10903); `a21_` 5 of 5; v13 alone 3 of 3 a side (0f65a665 7.1, 7.4, 8.4 s; a64549cc 7.8, 10.3, 5.7 s; load 57-60) |
| r365, 05:14Z | both, needs cpus=4 | `session_reads::` 10 rounds a side, the order alternated: 4 of 4 tests each on both sides |
| r369, 05:16Z | both, needs cpus=8 | replicated suites rounds 4-6 (below) |
| r392, 05:24Z | both, needs cpus=8 | t_t1_10_r5 alone beside six busy loops, 40 a side: 0 failures on either side; then 20 a side without: 0 |
| r335, 05:07Z | 0f65a665, needs cpus=2 | release bench built (271 s), V13's ladder, one round: every cell rc 0, no refusal, no deadline miss (below) |
| r346, 05:11Z | macOS, MacBook Pro 2 (needs cpus=8) | `a21_` 5 of 5; v13 alone, 3 rounds a side: failed 3 of 3 on 0f65a665 and 3 of 3 on a64549cc (above) |
| r402, 05:24Z | 0ba7307d and main 3a58a13f, needs cpus=2 | clippy clean (50 s); check-production rc 0 (10903); `a21_` 4 of 5 (the term change); v13 alone 3 of 3 a side (0ba7307d 6.2, 7.7, 6.8 s; 3a58a13f 8.3, 9.3, 6.9 s; load 47-57); suites, one round (below) |
| r410, 05:29Z | 0ba7307d, needs cpus=2 | release bench built (118 s), ladder, one round: every cell rc 0, no refusal, no deadline miss (below) |
| r432, 05:33Z | 63017ae8, needs cpus=2 | clippy clean (45 s); `a21_` 5 of 5, 10 rounds |
| r439, 05:35Z | a12bea22 (main 1f4a8e88), needs cpus=4 | clippy clean (36 s, engine re-checked); check-production rc 0 (10903); built 68 s; `a21_` 5 of 5, 3 rounds; v13 alone passes once (load 49) |
| r457, 05:39Z | cdcdef9f (main 731240ed), needs cpus=2 (cpus=4 refused: 3 of 32 cores idle) | clippy clean (41 s); check-production rc 0 (F6's new ratchet, 10617); built 71 s; `a21_` 5 of 5, 3 rounds; v13 alone passes |
| r478, 05:44Z | e53b671b (main ac2c429a: T21's optimizer/lip.rs), needs cpus=2 | clippy clean (29 s); built 61 s (engine compiled); `a21_` 5 of 5, 3 rounds; v13 alone passes |
| r497, 05:46Z | 8f59e5b8 (main 219603df: Q8's wal.rs), needs cpus=2 | clippy clean (38 s, engine re-checked); built 69 s (engine compiled); `a21_` 5 of 5, 3 rounds; v13 alone passes |

Ladder (`ladder13.sh`, Amphora alone, spread writers, one round each; commits/s, p50/p99 ms, refused by node, misses),
0f65a665 (r335, load 41-44) and 0ba7307d (r410, load 62-66, two cells' drivers also printed "node N closed the
connection" after the run):

| cell | 0f65a665 commits/s | p50/p99 | refused | misses | 0ba7307d commits/s | p50/p99 | refused | misses |
|---|---|---|---|---|---|---|---|---|
| persisted 32 | 37.2 | 754/1409 | 0, 0, 0 | 0 | 14.2 | 1737/3113 | 0, 0, 0 | 0 |
| replicated 32 | 36.8 | 688/1376 | 0, 0, 0 | 0 | 16.0 | 1409/3015 | 0, 0, 0 | 0 |
| persisted 128 | 31.8 | 3867/5308 | 0, 0, 0 | 0 | 19.8 | 6095/8258 | 0, 0, 0 | 0 |
| replicated 128 | 38.4 | 2327/4456 | 0, 0, 0 | 0 | 19.6 | 4588/9044 | 0, 0, 0 | 0 |

**The replicated suites** (FAILED only; `--test-threads=8`, 2 on r402). ckpt = `a_checkpoint_pass_copies_no_frame_past_the_last_finished_flush`,
image = `a_local_read_open_across_an_image_install_sees_one_side_whole`, room = `past_its_room_a_replica_refuses_submits_and_restarts_whatever_its_share`,
dblog = `a_replicas_database_log_stays_within_its_room`, holds = `a_local_read_holds_off_the_logs_restart_and_one_after_waits_for_it`,
due = `a_restart_due_waits_for_a_reads_deadline_and_no_longer`, nemesis = `every_falsifier_of_the_floor_holds_under_a_nemesis`,
flooded = `window_slow_flooded_fast_starved`, r5 = `session_reads::t_t1_10_r5_…`, term = `a21_…_term_change`:

| round (load) | the change | main |
|---|---|---|
| 1 (r312) | ckpt, image, dblog, nemesis, room | ckpt, image, room, holds |
| 2 (r312) | image, room, holds, due | ckpt, image, dblog, room, holds |
| 3 (r312) | image, dblog, room, r5 | ckpt, image, room, dblog, flooded, nemesis |
| 4 (r369) | ckpt, image, dblog, room | ckpt, holds, image, dblog, due, room |
| 5 (r369) | ckpt, holds, image, dblog, room | ckpt, image, dblog, room |
| 6 (r369) | ckpt, image, due, room | ckpt, image, dblog, room |
| r402 round 1 (0ba7307d, main 3a58a13f) | term, ckpt, image, dblog, room | ckpt, image, dblog, holds, room, flooded |

Rounds 1-6 on 0f65a665 and main a64549cc; the last on 0ba7307d and main 3a58a13f. v13 passed in all 12 suite logs of rounds
1-6 (grepped for its "ok"); r402's two suite runs list no v13 failure.

## `t_t1_10_r5` is the old `t_t1_10_r1` renamed; its failure is the test's own race, on main too (perf-A24)

**Lane times.** First command 05:50:25Z; the hour ends 06:50:25Z. Branch `perf/cross-a24`, from origin/main c205dd7d
(A21's charge landed). Ada's benchmark window on adacachy2 began at about 05:56Z: r538 (a build) had ended, and nothing
of this lane ran there after it. The other runs are on macOS (MacBook Pro 2, load 46-76 of other lanes).

**Losses.**
- **The count the brief asked for is not done.** The suite rounds that include r5, alternated, on Linux, wait for the
  window's end. The evidence here is forced, not counted: a reader started late on purpose (below). To see a rate of 1
  in 6 once takes 17 rounds a side with probability 95.5% (1-(5/6)^17; 26 rounds: 99.1%; A23's 6 had 66.5%).
- **Not merged.** Linux clippy, check-production, and the a21_ and session_reads tests on Linux wait for the window's
  end too. main has moved inside crates/ since c205dd7d (fab12d7e: B21, R12); perf/cross-a24 merges clean with it
  (`git merge-tree`), unchecked.
- **No natural failure was caught with stamps in it.** On macOS the follower comes back 19 to 303 ms after the reader's
  spawn, so a reader doesn't start that late there; the natural failures were on Linux, whose /tmp is RAM. What a natural
  failure's counters say is below, from the code.
- r557 was stopped by this lane after its two builds and one test run (in the table). The first RAM-disk probe (r5 on a
  macOS RAM disk) failed opening its files (`enforced Unix ownership` unavailable) and is not used, nor are two probes of
  the plain a23 binaries.
- Forced runs are 2 a cell, on one Mac, at load 46-76.

**r5 is `t_t1_10_r1` renamed.** 358fbde2 (2026-11-24 02:10Z) renamed `t_t1_10_r1_a_session_reads_its_write_on_a_lagging_follower`
to `t_t1_10_r5_...` (R1 and R2 stay with the cross-group tests it collided with) and changed nothing in its body. So r5 was in main's known
flaky set under its first name. Failures on trees with no line of A21's charge, from the records (the suites' rounds
that list r1 as failed, not a controlled count):

| run | tree | failed rounds |
|---|---|---|
| r1619 (V11) | main 98bd0fa9; eb365f3d | 1 of 6; 0 of 6 |
| r1660 (V11) | ad4c0922; eb365f3d | 0 of 6; 0 of 6 |
| r1756 (A17) | 848a16c2; cf13e776 (`database::`, 8 threads) | 1 of 1; 0 of 1 |
| r612, r628 (K) | 37008250 and its successor; main 1e874db1 | 1 of 3; 0 of 2 |
| r1896 (V13) | c3ebea5d, main's product with its new test; 2332e2ae | 1 of 3; 0 of 3 |
| r150 (A21) | main 7de5344d | 0 of 1 |
| r312, r369, r402 (A23) | main a64549cc; main 3a58a13f | 0 of 6; 0 of 1 |
| the same with A21's charge | d0f6a606 (r150); 0f65a665 (r312, r369); 0ba7307d (r402) | 1 of 1; 1 of 6; 0 of 1 |

K's and A23's failures counted `at_once: 1, waited: 0`; A17's and r312's panicked at session_reads.rs:122, the assertion
`counters.waited == 1`, the same line in 848a16c2, 0f65a665 and c205dd7d.

**What the test promises.** A session that wrote 2 reads it on a follower that has not applied it: the read is never answered
without 2, waits there (counted) while the follower is behind, and answers 1 and 2 once the follower rejoins; a read
with no token answers at once. A client observes the answer [1, 2] and that a read with no token doesn't wait. In every
failure whose panic line is known (r312's, A17's, all the forced ones) the answer was right: `assert_eq!(served, Here([1, 2]))`
comes first and passed. The failing step is the third, `waited == 1`: the test's own proof that the read met a follower that was behind.

**Where it goes wrong.** session_reads.rs:112-113, in the test: it spawns the reader and at once brings the follower back
(`rejoin`); nothing orders the two. `at_once: 1, waited: 0` beside a `Here` answer needs the reader's first check
(`wait_token`) to find the follower's published point at the token. The follower can't publish 2 before `rejoin`: until
then its records are lost, and only the main thread steps it. So in those failures the reader's first check came after
`rejoin` had begun. To show the order alone decides it, the test took stamps (the reader's first line, the follower back,
the read answered) and the follower's published point at the reader's first line, and the reader was started late on
request (`A24_READER_DELAY_US`), on main's tree and on the change's (macOS, branches diag/cross-a24-base,
diag/cross-a24-change):

| run | tree | reader started late by | each run: the reader's first line after its spawn, the follower back (µs), result |
|---|---|---|---|
| r557 | main ecdb013c + stamps (9a473313) | 0 | 84, 303363 pass |
| r606 | the same | 0 | 78, 184110 pass; 38, 25130 pass |
| r606 | the same | 150 ms | 223253, 161617 FAIL; 221298, 40866 FAIL |
| r606 | the same | 1 s | 1070102, 251919 FAIL; 1015816, 54809 FAIL |
| r606 | the same | 3 s | 3066390, 54869 FAIL; 3068208, 43603 FAIL |
| r606 | c205dd7d, A21's charge + stamps (a4eb33f2) | 0 | 1458, 47274 pass; 165, 269555 pass |
| r606 | the same | 150 ms | 220489, 213355 FAIL; 199644, 238921 pass |
| r606 | the same | 1 s | 1074014, 135968 FAIL; 1075047, 19152 FAIL |
| r606 | the same | 3 s | 3071428, 96320 FAIL; 3060578, 108990 FAIL |

The test failed exactly when the reader's first line came after the follower was back, 11 of the 17 runs, on main without
the change and on the change alike. In every failure the follower's point at that line was the write's own token (index 4),
the counters were `at_once: 1, waited: 0`, and the answer was [1, 2]. A21's charge is not in the read's path (`read_session`
calls no `enqueue`).

**The fix (536f9730; test only, the product outside `#[cfg(test)]` unchanged).** The test brings the follower back
only once the read waits there. `wait_token` counts a read that begins to wait and tells it under the closed time's lock
(`#[cfg(test)]`, as the disk switch's `held` does), so no publication falls between the count and the wait; the test
waits on that count (`Client::session_waits`), woken by the read. Its assertions are unchanged. The same stamps and
delays on the fix (macOS, diag/cross-a24-fix):

| run | tree | reader started late by | each run: the reader's first line after its spawn, the follower back (µs), result |
|---|---|---|---|
| r607 | the fix + the same stamps and delay (dde71900) | 0 | 49, 63380 pass; 45, 77806 pass |
| r607 | the same | 150 ms | 223529, 285660 pass; 214822, 252192 pass |
| r607 | the same | 1 s | 1074615, 1091835 pass; 1074679, 1101170 pass |
| r607 | the same | 3 s | 3066163, 3096021 pass; 3072204, 3100007 pass |

The follower came back after the reader's first line in all 8, its point there was still index 3, and the counters were
`at_once: 0, waited: 1`. On macOS at the fix's test binary (dde71900), `a21_` 5 of 5 and `session_reads::` 4 of 4, three
runs each (r638); `cargo fmt --all --check` clean; clippy -p amphora --all-targets clean on macOS (r635, 763 s, 60b50f0d: the fix's tree before
it was squashed into 536f9730).

| run | where | what |
|---|---|---|
| r538, 05:54Z | adacachy2, needs cpus=8 | built the stamps' lib tests on ecdb013c (fa44d54a, 59 s) and c205dd7d (e7a70c64, 24 s); kept as a24-tests-<sha> in ~/.cache/amphora-perf-bin/cross; ended before the window |
| r557, 05:57Z | macOS, needs cpus=8 | the same with the late reader (9a473313 679 s, a4eb33f2 332 s); stopped |
| r606, 06:16Z | macOS, needs cpus=6 | the forced runs on both trees |
| r607, 06:16Z | macOS, needs cpus=4 | built dde71900 (597 s), the forced runs on the fix |
| r635, 06:26Z | macOS, needs cpus=6 | clippy at 60b50f0d (the fix's tree): rc 0, 763 s |
| r638, 06:27Z | macOS, needs cpus=4 | a21_ and session_reads:: |

**For the next lane (Linux, once the window ends).** Scripts in bench/records/2026-11-23-adacachy2-perf-cross-a24/ (a24build.sh,
a24suite.sh, a24gates.sh, and a24m2.sh for macOS), run with `on` 00aad8daff2fa3c9daa65d53c425d450, sync false, `git show` them
out of `perf/cross-a24` in ~/.cache/amphora-perf/cross-remote:

1. Gates on the fix merged with main: `JOBS=8 a24gates.sh perf/cross-a24:SHA OUT` (clippy -p amphora --all-targets, check-production,
   `a21_` and `session_reads::` 3 times).
2. The count: `JOBS=8 a24suite.sh 17 OUT fa44d54a84126cba36bbba1e9b64a78b2d45f187 e7a70c6441e137d5eb7d1737062d20a0fb83e8fc 1` (the
   suites, main then A21's change, alternated; the binaries print each r5 run's stamps), START=2 beside it for a second stream; then
   the same number of rounds on the fix (`a24build.sh perf/cross-a24:SHA`). A23's rounds took 40 to 147 s at 8 threads.
