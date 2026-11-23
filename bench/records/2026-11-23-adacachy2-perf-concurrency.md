# 2026-11-23 adacachy2: perf/concurrency (AUDIT 6)

## Event-driven owner accept (transport/common.rs `Owner::accept`)

Verified: the owner's one accept loop (the local endpoint every attached call reaches, and the
network endpoint every dialed RPC reaches) polled a nonblocking listener and slept 10 ms on each
WouldBlock, so a connection arriving at an idle owner waited for the sleep to end.
Change: blocking accept; stopping wakes it by shutting the listening socket (Linux) or a loopback
connection (macOS, Windows).

Command (runner 00aad8daff2fa3c9daa65d53c425d450, release, 4 runs each, 2000 sequential connections,
host loaded by other lanes' builds, 0 of 32 cores idle):
`cargo test --release -p amphora --lib audit6_measure_accept -- --ignored --nocapture`

| | p50 | p99 | p99.9 | max | connections/s |
|---|---|---|---|---|---|
| before (99b66044) | 10132-10152 us | 10764-11146 us | 11224-12255 us | 11.6-12.7 ms | 98-99 |
| after | 57-63 us | 391-906 us | 776-2464 us | 0.9-3.2 ms | 8016-11792 |

p50 about 170x, sequential connections/s about 115x. The first after run is the coldest (8016/s).

## Descriptor exhaustion waits for the budget's give-back (perf-I2, item 4)

Verified: on EMFILE/ENFILE `Owner::accept` recounted and then `park_timeout(10 ms)`, a timer.
Change: `Descriptors` counts give-backs (`Held::drop`) and unparks the threads in
`wait_release`; the loop reads the count before each accept and, refused while the budget holds
descriptors, parks until a give-back after it (or stopping). With the budget holding none, the
park stays: no give-back will come and a host closing its own file raises no event (a decision).
Found on the way: at EMFILE the branch's recount (`refused_by_system`) opens /dev/fd, refused by
the same limit, and its error ended the listener before it reached any wait. Fixed: a recount
refused for want of a descriptor leaves the count for later. A/B with
`transport::common::tests::audit6_at_the_descriptor_limit_the_listener_waits_for_a_give_back`
(ignored; child process, RLIMIT_NOFILE lowered to leave 3 descriptors): before, the listener
stopped and the waiting connection never answered; after, it answered 69-150 us after the held
connection ended (5 runs: 96, 69, 150, 74, 117 us). Run beside the module's suite it failed once,
unexplained, hence ignored.

## Verified, not changed (perf-I2, for the successor)

- Item 1 (one proposal per group): `pending: Option<Pending>` (replicated.rs ~921) and
  `limits.proposals = 1` (~2602) hold. The actor prepares one transaction on the committed base
  ("The captured base cannot be used by another write until this one applies", `expire`), so
  batching means preparing several waiting submits as one entry (each under its own savepoint,
  answered apart) and pipelining means preparing on an uncommitted base: an engine change, not
  a limit change. Not attempted inside the hour.
- Item 2: `deadline()` scans every waiting envelope each step and `expire()` rotates the whole
  `waiting` queue each step (O(N) per step, N up to `command_bound()`). Holds.
- Item 3: `Listener::accept` and `Dialer::connect` (transport/network.rs) build a fresh
  `ServerConfig`/`ClientConfig` and `RootCertStore` per connection. Holds. Caching them would also
  turn on rustls's in-memory TLS session resumption for the dialer (a behaviour change to decide).

## The descriptor-limit test's module-run failure explained (perf-I3)

Cause: Linux's accept reserves its descriptor before it waits for a connection (`do_accept` takes
an unused descriptor first). The test let its first client connect before making the second's
socket, so a listener that took the first and went back to accept ahead of the test thread held
the last number below the lowered limit, and the second's socket was refused (EMFILE) at its
`unwrap`. Alone, the test thread nearly always won that race; beside the module's suite, under
load, it lost once. Not a product bug: the clients are the test's own, in the listener's process.
Change: both client sockets are made before the first connects, and the test is no longer ignored.

Forced (the race made likely: the child pinned to one core, `taskset -c 0`, 20 runs each):
`taskset -c 0 cargo test -p amphora --lib --locked <test> -- --include-ignored --exact`

| | passed | failed |
|---|---|---|
| after (this change) | 20 | 0 |

`transport::common` module run with the test included, 5 runs: 15 of 15 each (1 measurement ignored).

## The owner's waiting submits ordered by deadline (perf-I3, AUDIT 6 item 2)

Verified: `deadline()` scanned every waiting submit each step, and `expire()` popped and pushed
back every one each step, O(N) a step with N up to `command_bound()`.
Change: `database/replicated/waiting.rs`: the waiting line keeps each submit by its place in line
(a `BTreeMap`) and by its deadline (a `BTreeSet`, ties in line order). The soonest deadline is the
set's first, the expired are taken from its front, pop/push are O(log N); the answers are the
same (FIFO dispatch, a requeued submit first, expiry at `at >= until`, `Timeout` answers).

Command (runner 00aad8daff2fa3c9daa65d53c425d450, release, 20,000 steps each, none due, both sides
in each run, 6 runs; host load average 50-64 on 32 cores from other lanes):
`cargo test --release -p amphora --lib audit6_measure_waiting -- --ignored --nocapture`

| waiting | before (scan + rotate), median (range) | after, median (range) |
|---|---|---|
| 1 | 5 ns/step (3-15) | 2 ns/step (1-2) |
| 100 | 758 ns/step (217-903) | 3 ns/step (2-4) |
| 1,000 | 3,937 ns/step (2,146-5,575) | 4 ns/step (2-4) |
| 10,000 | 36,950 ns/step (21,787-37,896) | 5 ns/step (2-5) |

What it measures: the owner's per-step work on its waiting submits alone (the old loop's shape
against the new line), not a whole step; the rest of a step doesn't depend on N. Before grows
linearly (2.2-3.8 ns a waiting submit a step); after is flat at the sizes asked.
