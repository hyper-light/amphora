# A subquery runs where it is read: the suite's subquery statements, before and after

The lazy-subquery change's step 4 (kiln plan 01a1700672f7773295fbe1e185206fb7; CLAUDE.md §7;
`docs/design/executor.md`, decision 6), on the machine Ada chose on 2026-10-25, fedora being drained
for Quiver's grid.

- **Hardware:** adas-mac-studio: Apple M4 Max, 14 cores, 36 GiB, APPLE SSD AP0512Z (APFS), macOS 26.2
  (Darwin 25.2.0). A virtual machine process (30 to 70% of a core) and the EXO app ran throughout:
  the machine idles at a load of 1.7 to 2.9.
- **Binaries:** amphora-bench in release, from 175edac (before: the change's parent) and c1c0c7d
  (after: the change), each from a clean checkout (`dirty_files` 0 in every record), with one
  `bench/Cargo.lock`.
- **Runs:** before and after in turns, each started once the load average was under 2: 1.72 to 1.98
  at every start but one, round 4's before run of the second record, which started at 2.17 after the
  ten-minute wait. `bench/run.sh` reads Linux's `/proc`, so a wrapper made each record with run.sh's
  fields from `amphora-bench --target amphora --dir ~/.cache/amphora-bench-data/amphora --suite
  --items 200000 --clients 1,32 --seconds 5 --warmup 1 --only 'subquery_in$,exists_subquery$'`.

## Best of 3: `2026-10-25-adas-mac-studio-lazy-subqueries.jsonl`

2026-10-25, 00:37 to 00:58Z; each side's highest throughput and lowest median across its three runs.

| statement | clients | before /s | after /s | after/before | before p50 ms | after p50 ms | after/before |
|---|---:|---:|---:|---:|---:|---:|---:|
| exists_subquery | 1 | 534 | 541 | 1.013 | 0.444 | 0.444 | 1.000 |
| exists_subquery | 32 | 772 | 769 | 0.996 | 10.496 | 11.136 | 1.061 |
| subquery_in | 1 | 906 | 899 | 0.993 | 1.072 | 1.072 | 1.000 |
| subquery_in | 32 | 1,307 | 1,319 | 1.009 | 21.760 | 20.736 | 0.953 |

## exists_subquery again, five rounds of 10 s: `2026-10-25-adas-mac-studio-lazy-subqueries-exists.jsonl`

2026-10-25, 01:02 to 01:37Z, with `--seconds 10 --warmup 2 --only 'exists_subquery$'`.

| clients | before /s, best | after /s, best | before /s, median run | after /s, median run | before p50 ms, runs | after p50 ms, runs | p50, median run after/before |
|---:|---:|---:|---:|---:|---|---|---:|
| 1 | 559 | 554 | 534.6 | 550.2 | 0.452 to 0.500 | 0.460 to 0.468 | 0.975 |
| 32 | 769 | 766 | 764.7 | 763.9 | 9.728 to 10.624 | 10.112 to 10.368 | 0.988 |

p99 at 32 clients: 299 to 311 ms before, 299 to 307 ms after.

## Reading

The change costs a correlated EXISTS read at each row a Goto, a Gosub and a Return, and an
uncorrelated IN one Once a read. Time per statement (before's throughput over after's) is within 1%
for both statements at both client counts (the slowest 1.007x, subquery_in at 1 client), and a
statement's median time alone (1 client) is unchanged: 1.000 for both.

The one figure past 2% is exists_subquery's median at 32 clients in the best of 3, 10.50 to
11.14 ms (1.061). At 32 clients on 14 cores that statement waits on its queue (its median over 20 times
its time alone, its p99 300 ms), and its median moves more from run to run than the bar: in the
second record the unchanged binary's five medians spread from 9.73 to 10.62 ms, and the changed
binary's lay inside them (10.11 to 10.37 ms; the median run 10.24 against 10.37 ms, 0.988), with
throughput 763.9 against 764.7 statements a second at the median run and p99 the same.

No subquery statement is slower by more than 2%: time per statement within 1%, the median alone
unchanged, and the median at 32 clients within its own run-to-run spread.
