# perf/pagepath: point reads, AUDIT 2.1 and 2.3 (2026-11-23, adacachy2)

Host: adacachy2 (runner 00aad8daff2fa3c9daa65d53c425d450), 32 cores, x86_64 Linux. NOT quiet: load
average 60-87 from other work through every run (CLAUDE.md §7 asks under 2); the ranges below are that
noise. Base: origin/main b817508b. Release bench binary (`cargo build --release --manifest-path
bench/Cargo.toml`), each binary copied aside, the same database (`--reuse`), base and after
alternating, three reps each:

    amphora-bench --target amphora --dir <data>/amphora --suite --reuse \
      --only 'point_pk$,point_secondary$' --clients 1,8,32,64,128 --seconds 5 --warmup 1 --items 200000

(script: perf/pagepath's `.git/perf-ab.sh`, records `~/.cache/amphora-perf-bin/pagepath/ab-0051.jsonl`
on adacachy2). Queries a second, median of 3 [min-max]; SQLite one run, same binary and client.

## AUDIT 2.1. WAL header publication and lock padding on the default path

The change landed as `e74813c8`. The original record labels the measured after binary
`bf23ccb1`; that object is absent from the fetched history. The runs below exercised
`PagePath::Today` with `LOG_READ_INDEX=locked`. The table retains throughput only.

| op | clients | base | after | after/base | SQLite |
|---|---|---|---|---|---|
| point_pk | 1 | 192,905 [159,502-279,074] | 249,427 [185,384-310,574] | 1.29 | 523,323 |
| point_pk | 8 | 1,048,047 [345,875-1,060,594] | 1,004,391 [992,818-1,214,045] | 0.96 | 769,260 |
| point_pk | 32 | 829,257 [808,473-913,400] | 881,652 [740,888-995,658] | 1.06 | 261,247 |
| point_pk | 64 | 809,632 [696,891-868,279] | 921,433 [878,925-928,751] | 1.14 | 225,140 |
| point_pk | 128 | 790,786 [761,445-792,039] | 838,570 [822,718-843,976] | 1.06 | 185,441 |
| point_secondary | 1 | 235,117 [167,361-241,638] | 247,138 [240,215-254,132] | 1.05 | 207,777 |
| point_secondary | 8 | 754,345 [748,200-906,522] | 860,024 [633,072-982,555] | 1.14 | 91,092 |
| point_secondary | 32 | 756,335 [599,812-857,805] | 674,282 [588,109-753,973] | 0.89 | 55,781 |
| point_secondary | 64 | 670,076 [439,000-741,801] | 705,136 [573,247-746,240] | 1.05 | 47,730 |
| point_secondary | 128 | 453,377 [389,654-468,188] | 439,142 [435,431-452,205] | 0.97 | 45,469 |

Reading: no regression outside the noise; point_pk at 64 and 128 clients is above base's whole range
(+14%, +6%). The 10x isn't here: the header was already read lock-free under the non-default read
indexes (decision 40), and the default's per-read cost is elsewhere (AUDIT 2.1's frame_cache lock,
2.2). To re-measure on a quiet host.

## AUDIT 2.3. Single shared-page lookup and shard selection

The distribution test in `a02e4b20` checks the first pages of 64 files and consecutive pages of
one file. This record contains no paired timed run of the changed `PagePath::Plain` lookup.
