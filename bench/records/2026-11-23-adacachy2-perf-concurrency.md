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
