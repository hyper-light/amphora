# A subquery the output reads is priced once an output row: before and after

The planner change's step 4 (kiln plan 01a1748ba15c7791a567367d0740c941; CLAUDE.md §7;
`docs/design/planner.md`, decision 9), on the machine Ada named on 2026-10-25.

- **Hardware:** adas-mac-studio-2: Apple M4 Max, 14 cores, 36 GiB, APPLE SSD AP0512Z (APFS), macOS
  26.2 (Darwin 25.2.0). FaceTime held one core at 99% throughout and a virtual machine process a fifth
  of another, so the machine idles at a load near 2.
- **Binaries:** amphora-bench in release, each from a clean checkout (`dirty_files` 0 in every
  record), with one `bench/Cargo.lock`: d4f6671 (before: the change's parent) and d478ff6 (after: the
  change).
- **Runs:** before and after in turns, three rounds, each run started once the load average was
  under 2 (1.65 to 1.99). `bench/run.sh` reads Linux's `/proc`, so a wrapper made each record with
  run.sh's fields from `amphora-bench --target amphora --dir ~/.cache/amphora-bench-data/amphora
  --suite --items 200000 --clients 1,32 --seconds 5 --warmup 1 --only
  'group_correlated$,group_correlated_row$,exists_subquery$'`.

## Best of 3: `2026-10-25-adas-mac-studio-2-output-subquery-calls.jsonl`

2026-10-25, 20:19 to 20:38Z; each side's highest throughput and lowest median across its three runs.

| statement | clients | before /s | after /s | after/before | before p50 ms | after p50 ms | after/before |
|---|---:|---:|---:|---:|---:|---:|---:|
| exists_subquery | 1 | 539 | 540 | 1.002 | 0.432 | 0.432 | 1.000 |
| exists_subquery | 32 | 777 | 775 | 0.997 | 10.496 | 10.496 | 1.000 |
| group_correlated | 1 | 227 | 26,895 | 118.375 | 4.352 | 0.036 | 0.008 |
| group_correlated | 32 | 1,231 | 225,470 | 183.160 | 22.016 | 0.046 | 0.002 |
| group_correlated_row | 1 | 148,812 | 149,756 | 1.006 | 0.006 | 0.006 | 1.000 |
| group_correlated_row | 32 | 785,387 | 774,928 | 0.987 | 0.014 | 0.014 | 1.000 |

The runs, lowest to highest:

| statement | clients | before /s | after /s | before p50 ms | after p50 ms |
|---|---:|---|---|---|---|
| exists_subquery | 1 | 535 to 539 | 529 to 540 | 0.432 | 0.432 to 0.444 |
| exists_subquery | 32 | 772 to 777 | 770 to 775 | 10.496 to 10.624 | 10.496 to 10.624 |
| group_correlated | 1 | 221 to 227 | 25,416 to 26,895 | 4.352 to 4.480 | 0.036 to 0.038 |
| group_correlated | 32 | 1,227 to 1,231 | 220,010 to 225,470 | 22.016 to 22.784 | 0.046 to 0.047 |
| group_correlated_row | 1 | 145,538 to 148,812 | 144,946 to 149,756 | 0.006 | 0.006 |
| group_correlated_row | 32 | 778,994 to 785,387 | 758,771 to 774,928 | 0.014 | 0.014 to 0.015 |

The programs (EXPLAIN on the suite's database through `amphora-bench --query --explain`, both
release builds): `exists_subquery`'s and `group_correlated_row`'s are the same before and after.
`group_correlated`'s changes from the unnested form, a LEFT JOIN on the items of every group grouped
into an ephemeral index (99 instructions), to the correlated form, a search of its one group's
entries of `items_grp_created` from the group's output (66).

## Reading

`group_correlated` runs its subquery once for its one group instead of grouping all 200,000 items
first: time per statement 0.008x at 1 client (4.35 to 0.036 ms) and 0.005x at 32, 118 and 183 times
as many statements a second. The controls run the same programs before and after and measure
within 2% in time per statement (at most 1.013x, `group_correlated_row` at 32 clients). Its three
runs after (758,771 to 774,928 a second) lie under its three before (778,994 to 785,387) on the
same program, the spread of 780,000 statements a second from 32 clients on 14 cores.
