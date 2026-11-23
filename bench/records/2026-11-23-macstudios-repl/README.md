# Replicated commits on three Mac Studios: not run, the Macs are on Wi-Fi (lane S5, 2026-11-23)

Ada asked for Amphora against rqlite with each member on its own Mac Studio (cec37f60, b24c5c95,
2768d0d5) and the client on Mac Studio 2 (d1323898), to test AC-T1.8.3 (p99 at most 2 ms for 3
nodes on a LAN) with real fates. The timed runs were not started: every one of the four Macs reaches
the others over Wi-Fi, and the network's own round trip is above the 2 ms target before any database
work.

## What the network measured

All four Macs route 192.168.1.0/24 through `en1` (Wi-Fi). `en0` (Ethernet) is `status: inactive,
media: autoselect (none)` on every one: no cable link.

`ping -c 20 -i 0.1`, one Mac at a time, nothing else of this lane running (18:33Z):

| from | to | min ms | avg ms | max ms | stddev ms |
|---|---|---|---|---|---|
| Studio 3 (.84, cec37f60) | .81 | 5.5 | 126.4 | 588.4 | 144.7 |
| Studio 3 (.84) | .83 | 7.0 | 68.5 | 248.0 | 77.1 |
| Studio 3 (.84) | .82 client | 6.9 | 99.5 | 489.4 | 115.8 |
| b24c5c95 (.81) | .84 | 7.4 | 237.4 | 1283.6 | 311.3 |
| b24c5c95 (.81) | .83 | 6.1 | 83.5 | 521.7 | 135.9 |
| b24c5c95 (.81) | .82 client | 8.1 | 354.2 | 1483.6 | 434.0 |
| Studio 4 (.83, 2768d0d5) | .84 | 5.9 | 111.6 | 339.4 | 114.8 |
| Studio 4 (.83) | .81 | 6.9 | 66.6 | 199.8 | 71.3 |
| Studio 4 (.83) | .82 client | 5.9 | 98.9 | 480.5 | 144.7 |
| Studio 2 (.82, client) | .84 | 7.4 | 90.9 | 465.1 | 122.6 |
| Studio 2 (.82) | .81 | 6.4 | 64.1 | 232.8 | 61.0 |
| Studio 2 (.82) | .83 | 5.1 | 22.1 | 90.8 | 22.0 |

Bulk transfer over the same links: 440-520 KB/s (`curl` of a 52 MB file from the client Mac).

A quorum commit costs at least two round trips (client to a member, leader to a follower and back,
answer to the client): at the 5-8 ms minimum that is 10-16 ms, five to eight times AC-T1.8.3's
p99. The tails (max 0.2-1.5 s, stddev up to 434 ms) would set every p99 and p99.9 in a 5-second
run, for both systems alike. A run here would measure the Wi-Fi, not the write paths (benchmark
checklist, question 1: the limiter is known before the run).

## What is ready for the rerun once the Macs are wired

- Amphora at main `a3642e93` built for macOS on Mac Studio 2 with the shipping profile
  (`CARGO_PROFILE_RELEASE_LTO=thin`, `CODEGEN_UNITS=1`, `OVERFLOW_CHECKS=true`; the bench workspace's
  own release profile sets none of them): `~/.cache/amphora-s5-bin/amphora-bench` there, sha256
  `c2a41ade9af3c9458ed975c9c8c0924840a8d821475427965cb23154cea5238e`. Target dir
  `~/.cache/amphora-perf-target/bench-s5`.
- rqlite v10.7.4 has no darwin release asset (its release lists linux, win64, rpm and deb only).
  rqlited was built on Mac Studio 2 from the v10.7.4 source tag (`archive/refs/tags/v10.7.4.tar.gz`,
  sha256 `41e8653d468063fb061f7d57bd9ef77446d08e7a541aee22dfa2046f746dd7cc`) with go1.27.4,
  CGO_ENABLED=1: `rqlited v10.7.4 darwin arm64 go1.27.4 sqlite3.54.1`. Same place, sha256
  `737f2d276f924f1d0d15685d8b80d72244b60519a88f7ede10e6b594976b6714`.
- `rqlite.sh` here: amphora-bench's `--scripts` system script for rqlite's three nodes on three hosts
  (`RQ_IPS`, `RQ_PORT`), bash 3.2 (macOS's). `bench/servers/repl/lib.sh` is Linux only (setsid,
  pidfds, GNU tail).
- `s5.js` here: the kiln `code_run` program that runs the matrix: for each of 5 rounds (system order
  reversed every round), writers 1, 32, 128, each of amphora-persisted, amphora-replicated
  (`--spanning 2`, each member its own fate: node ID = host) and rqlite gets a fresh three-node cluster,
  one member a background kiln run on each Mac, the client `amphora-bench --repl commit` on Mac
  Studio 2 with S4's flags (`--batch 1 --text-bytes 600 --deadline-s 10 --relay-records 128
  --warmup-s 1 --seconds 5`), and every host's load and memory logged at each round's start and end.
  Records land in `~/.cache/amphora-s5/rec/round<N>/` on the client; each record's `by_node[].buckets_us`
  pools across rounds (W6). Input: `{"members": [{"id": "<runner>", "ip": "<wired ip>"} x3], "client":
  "<runner>", "aport": 29290, "rport": 29200, "rounds": [1,2,3,4,5], "ladder": [1,32,128]}` (ports
  below macOS's ephemeral 49152-65535). Not yet run end to end: smoke one pair first.
- Both systems flush with F_FULLFSYNC on macOS (inferred from source, not traced: Rust std's
  `sync_data` on Apple targets and Go's `File.Sync` on darwin both issue it; Amphora's replicated log
  calls `sync_data`, crates/amphora/src/database/replicated.rs; bbolt syncs through `File.Sync` off Linux).

The member Macs' scratch (`~/.cache/amphora-s5`) was removed. Free disk then: Studio 3 30 GB,
b24c5c95 42 GB, Studio 4 21 GB (Studio 4 also ran another agent's kiln gate tests, load 5-6 of 14).
The binaries' copies over Wi-Fi took minutes each (119 MB at ~0.45 MB/s), another reason to wire first.
