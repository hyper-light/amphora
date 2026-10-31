# A correlated subquery runs where it is read, in every query: before and after

The correlated-subquery change's step 6 (kiln plan 01a17298aafe775784507ddc088a5df6; CLAUDE.md §7;
`docs/design/executor.md`, decision 6), on the machine Ada named on 2026-10-25.

- **Hardware:** adas-mac-studio-2: Apple M4 Max, 14 cores, 36 GiB, APPLE SSD AP0512Z (APFS), macOS
  26.2 (Darwin 25.2.0). FaceTime held one core at 99% throughout and a virtual machine process a fifth
  of another, so the machine idles at a load near 2; other work ran on it at times (a kiln test, a
  Rust build, Spotlight's indexer), and the runs waited for it.
- **Binaries:** amphora-bench in release, each from a clean checkout (`dirty_files` 0 in every
  record), with one `bench/Cargo.lock`. The first record: 07d2de8 (before: the change's parent,
  b0905b0, with the suite's `group_correlated`) and 7472e5e (after: the change). 7472e5e was
  recommitted as d4efe6b to correct its author line, with the same tree (49c0355). The second record:
  fc48363 (before: 07d2de8 with `group_correlated_row`) and 45ef44b (after: d4efe6b with the same
  statement).
- **Runs:** before and after in turns, each started once the load average was under 2 (1.90 to 1.99),
  waiting up to four hours for it (57 minutes at most). Three runs of the first record started past 2
  when their shorter waits of the first passes ran out (3.41, 4.78 and 4.53) and are left out; they are
  in the record. `bench/run.sh` reads Linux's `/proc`, so a wrapper made each record with run.sh's
  fields from `amphora-bench --target amphora --dir ~/.cache/amphora-bench-data/amphora --suite
  --items 200000 --clients 1,32 --seconds 5 --warmup 1 --only <statements>`.

## The suite's subquery statements: `2026-10-25-adas-mac-studio-2-corr-subqueries.jsonl`

2026-10-25, 14:47 to 17:45Z, with `--only 'subquery_in$,exists_subquery$,group_correlated$'`; each
side's highest throughput and lowest median across the runs that count, six before and four after.

| statement | clients | before /s | after /s | after/before | before p50 ms | after p50 ms | after/before |
|---|---:|---:|---:|---:|---:|---:|---:|
| exists_subquery | 1 | 533 | 531 | 0.996 | 0.436 | 0.444 | 1.018 |
| exists_subquery | 32 | 766 | 764 | 0.997 | 7.296 | 10.368 | 1.421 |
| group_correlated | 1 | 218 | 218 | 1.000 | 4.544 | 4.544 | 1.000 |
| group_correlated | 32 | 1,184 | 1,190 | 1.005 | 21.504 | 23.296 | 1.083 |
| subquery_in | 1 | 900 | 913 | 1.014 | 1.088 | 1.072 | 0.985 |
| subquery_in | 32 | 1,296 | 1,293 | 0.997 | 17.920 | 21.760 | 1.214 |

The runs that count, lowest to highest:

| statement | clients | before /s | after /s | before p50 ms | after p50 ms |
|---|---:|---|---|---|---|
| exists_subquery | 1 | 442 to 533 | 523 to 531 | 0.436 to 0.544 | 0.444 to 0.452 |
| exists_subquery | 32 | 711 to 766 | 705 to 764 | 7.296 to 12.416 | 10.368 to 12.288 |
| group_correlated | 1 | 200 to 218 | 213 to 218 | 4.544 to 4.928 | 4.544 to 4.608 |
| group_correlated | 32 | 1,098 to 1,184 | 1,162 to 1,190 | 21.504 to 26.112 | 23.296 to 25.088 |
| subquery_in | 1 | 763 to 900 | 887 to 913 | 1.088 to 1.184 | 1.072 to 1.104 |
| subquery_in | 32 | 1,222 to 1,296 | 1,224 to 1,293 | 17.920 to 24.576 | 21.760 to 24.320 |

On the suite's database the three statements compile to the same program before and after (EXPLAIN
through `amphora-bench --query --explain`). `subquery_in`'s and `exists_subquery`'s subqueries already
ran where read (c1c0c7d). `group_correlated`'s count is unnested: the planner turns it into a LEFT JOIN
on the items grouped by grp, a scan of all 200,000 entries of `items_grp_created`, which its cost
model prefers to the correlated form, priced at one call an input row (kiln goal
01a17471b5c171b2a64a1bc33d826a6c). So this record measures the same programs twice: what moves
between the sides is the machine.

## The shape the change moves: `2026-10-25-adas-mac-studio-2-corr-subqueries-row.jsonl`

2026-10-25, 19:05 to 19:27Z, with `--only 'group_correlated_row$'`: `SELECT o.region, count(*),
(SELECT i.price FROM items i WHERE i.grp = o.region ORDER BY i.created DESC LIMIT 1) FROM owners o
WHERE o.region = ? GROUP BY o.region`, a correlated subquery that returns a row, which no join
replaces. Before, its body ran inline in the loop over the region's owners (177 for region 5), once
an input row; after, it is a subroutine the group's output calls once. Both builds return the same
row. Best of 3:

| clients | before /s | after /s | after/before | before p50 ms | after p50 ms | after/before |
|---:|---:|---:|---:|---:|---:|---:|
| 1 | 9,209 | 145,912 | 15.845 | 0.107 | 0.006 | 0.056 |
| 32 | 66,673 | 763,152 | 11.446 | 0.150 | 0.013 | 0.087 |

The runs: before 9,161 to 9,209 statements a second at 1 client and 65,435 to 66,673 at 32; after
145,612 to 145,912 and 752,151 to 763,152.

## Reading

Time per statement (before's throughput over after's) is at most 1.004x on the suite's three
statements (`exists_subquery` at 1 client, 533 against 531 a second), inside the bar of 1.02x, and
0.063x and 0.087x on the statement whose subquery the change moves: 15.8 times as many statements a
second at 1 client, 11.4 times at 32.

The medians at 32 clients differ by up to 1.42x between the sides of the best-of table, on programs
that are the same: 32 clients on 14 cores queue, and each side's lowest median is its luckiest run.
`exists_subquery`'s medians before spread from 7.30 to 12.42 ms, and the after's lie inside them, 10.37
to 12.29 ms. At 1 client, where nothing queues, the medians are within 2% (`exists_subquery` 0.436
against 0.444 ms; the others 1.000 and 0.985).

No subquery statement is slower: the suite's three run the same program before and after and
measure within 1% in time per statement, and the moved shape runs its subquery once a group instead
of once an input row, 11 to 16 times as fast.
