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
