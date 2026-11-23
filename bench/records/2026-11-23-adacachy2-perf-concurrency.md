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
