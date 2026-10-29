# One estimate per joined set: the bench suite before and after

Step 4 of the planner plan (kiln plan 01a16eab7f457673b4cf9933adacfc4d; `docs/design/planner.md`,
decision 13; CLAUDE.md §7): HEAD before the change (dbef789) against the change's last commit
(d762dc2), the whole suite.

- **Hardware:** adas-mac-studio-2: Apple M4 Max, 14 cores, 36 GiB, APPLE SSD (APFS), macOS 26.2. A
  virtual machine and other agents' work share it; each run waited for a 1-minute load under 2.
- **Binaries:** amphora-bench in release, each built from a clean export (`git archive`) of its
  commit, with its own target directory.
- **Runs:** `--target amphora --suite --items 200000 --clients 1,32 --seconds 5 --warmup 1`, four
  rounds in ABBA order (rounds 1 and 3 before first, 2 and 4 after first), 2026-10-26 13:48 to
  16:20Z, each started at load 1.82 to 1.98 (the suite's own 32 clients raised it to 3.7 to 5.1).
  The records' JSON (run.sh's fields) stayed on the machine; the table below is all of it.

## Plans and programs

`--suite --explain` with both binaries at 200,000 and at 20,000 items: the 52 statements' plans are
the same. The bytecode (`--query --explain` on the same database, both binaries) is the same for
every statement checked below 0.97 or near it: table_count, count_range, count_distinct,
group_correlated, group_correlated_row, point_text, point_secondary, point_pk_one_column,
range_secondary_100, filter_is_null, like_prefix, order_by_index, union_all, text_read. Statements
are prepared once a connection (`amphora::db::Db::query` keeps them), so the planner runs outside
the timed loop: the change's code doesn't run in any measured statement. The writes below 0.97
(insert_one, insert_batch_100, delete_range_insert) don't reach the join search at all.

## Best of 4 and median, statements a second

| statement | clients | before best | after best | best ratio | median ratio |
|---|---:|---:|---:|---:|---:|
| aggregates_range | 1 | 5,697 | 5,772 | 1.013 | 1.015 |
| aggregates_range | 32 | 8,202 | 8,574 | 1.045 | 1.044 |
| blob_read | 1 | 607,422 | 620,414 | 1.021 | 1.006 |
| blob_read | 32 | 826,865 | 808,826 | 0.978 | 0.982 |
| case_aggregate | 1 | 5,826 | 5,856 | 1.005 | 1.006 |
| case_aggregate | 32 | 7,991 | 8,387 | 1.050 | 1.044 |
| count_distinct | 1 | 461 | 465 | 1.010 | 1.012 |
| count_distinct | 32 | 601 | 638 | 1.061 | 1.043 |
| count_range | 1 | 3,574 | 3,447 | 0.965 | 1.102 |
| count_range | 32 | 16,019 | 17,597 | 1.098 | 1.087 |
| cte | 1 | 361 | 367 | 1.017 | 1.006 |
| cte | 32 | 597 | 631 | 1.057 | 1.042 |
| date_bucket | 1 | 369 | 373 | 1.011 | 1.001 |
| date_bucket | 32 | 839 | 877 | 1.046 | 1.037 |
| delete_insert | 1 | 195 | 199 | 1.022 | 1.017 |
| delete_insert | 32 | 4,141 | 4,378 | 1.057 | 1.047 |
| delete_range_insert | 1 | 209 | 194 | 0.929 | 0.956 |
| delete_range_insert | 32 | 1,089 | 1,062 | 0.976 | 1.010 |
| distinct_range | 1 | 628 | 635 | 1.010 | 1.000 |
| distinct_range | 32 | 816 | 859 | 1.052 | 1.042 |
| exists_subquery | 1 | 534 | 547 | 1.024 | 0.994 |
| exists_subquery | 32 | 742 | 785 | 1.057 | 1.057 |
| filter_is_null | 1 | 10,393 | 9,773 | 0.940 | 0.988 |
| filter_is_null | 32 | 13,427 | 14,295 | 1.065 | 1.052 |
| filter_multi_column | 1 | 1,614 | 1,658 | 1.027 | 0.988 |
| filter_multi_column | 32 | 1,888 | 2,012 | 1.066 | 1.057 |
| filter_no_index | 1 | 38 | 38 | 1.011 | 1.019 |
| filter_no_index | 32 | 38 | 44 | 1.146 | 1.005 |
| group_by_having | 1 | 393 | 405 | 1.030 | 0.987 |
| group_by_having | 32 | 593 | 629 | 1.060 | 1.043 |
| group_by_range | 1 | 468 | 473 | 1.009 | 1.028 |
| group_by_range | 32 | 808 | 850 | 1.051 | 1.045 |
| group_correlated | 1 | 24,423 | 26,147 | 1.071 | 1.099 |
| group_correlated | 32 | 227,474 | 230,916 | 1.015 | 1.002 |
| group_correlated_row | 1 | 134,029 | 137,621 | 1.027 | 1.038 |
| group_correlated_row | 32 | 794,756 | 783,490 | 0.986 | 0.988 |
| in_list_10 | 1 | 74,077 | 75,684 | 1.022 | 0.969 |
| in_list_10 | 32 | 115,310 | 120,352 | 1.044 | 1.039 |
| insert_batch_100 | 1 | 172 | 174 | 1.008 | 1.011 |
| insert_batch_100 | 32 | 643 | 595 | 0.925 | 1.325 |
| insert_one | 1 | 198 | 201 | 1.015 | 0.992 |
| insert_one | 32 | 6,573 | 6,257 | 0.952 | 0.935 |
| join_group | 1 | 83 | 87 | 1.041 | 1.026 |
| join_group | 32 | 118 | 123 | 1.044 | 1.048 |
| join_pk | 1 | 125,802 | 128,143 | 1.019 | 1.007 |
| join_pk | 32 | 313,296 | 332,080 | 1.060 | 1.035 |
| join_three | 1 | 2,386 | 2,451 | 1.027 | 1.011 |
| join_three | 32 | 8,478 | 8,973 | 1.058 | 1.039 |
| json_field | 1 | 473,119 | 481,129 | 1.017 | 1.012 |
| json_field | 32 | 820,161 | 805,951 | 0.983 | 0.983 |
| json_filter | 1 | 395 | 404 | 1.023 | 1.013 |
| json_filter | 32 | 664 | 706 | 1.064 | 1.044 |
| left_join_anti | 1 | 573 | 594 | 1.038 | 1.019 |
| left_join_anti | 32 | 718 | 754 | 1.049 | 1.043 |
| like_prefix | 1 | 234,169 | 235,707 | 1.007 | 0.978 |
| like_prefix | 32 | 844,031 | 821,955 | 0.974 | 0.983 |
| like_substring | 1 | 258 | 254 | 0.985 | 1.012 |
| like_substring | 32 | 425 | 425 | 1.000 | 1.045 |
| offset_deep | 1 | 93 | 92 | 0.987 | 0.946 |
| offset_deep | 32 | 81 | 84 | 1.034 | 1.025 |
| order_by_index | 1 | 201,198 | 201,530 | 1.002 | 0.985 |
| order_by_index | 32 | 779,640 | 769,691 | 0.987 | 0.978 |
| order_by_sort | 1 | 464 | 476 | 1.026 | 0.974 |
| order_by_sort | 32 | 594 | 621 | 1.046 | 1.044 |
| point_pk | 1 | 536,192 | 529,664 | 0.988 | 0.988 |
| point_pk | 32 | 814,184 | 799,631 | 0.982 | 0.987 |
| point_pk_one_column | 1 | 607,295 | 598,925 | 0.986 | 0.987 |
| point_pk_one_column | 32 | 831,296 | 806,262 | 0.970 | 0.984 |
| point_secondary | 1 | 452,139 | 432,033 | 0.956 | 0.993 |
| point_secondary | 32 | 798,977 | 781,074 | 0.978 | 0.971 |
| point_text | 1 | 412,041 | 389,048 | 0.944 | 0.964 |
| point_text | 32 | 765,258 | 755,340 | 0.987 | 0.975 |
| range_pk_100 | 1 | 61,099 | 60,368 | 0.988 | 0.985 |
| range_pk_100 | 32 | 76,210 | 80,049 | 1.050 | 1.043 |
| range_secondary_100 | 1 | 251,625 | 254,103 | 1.010 | 0.986 |
| range_secondary_100 | 32 | 870,930 | 842,792 | 0.968 | 0.979 |
| string_functions | 1 | 31,555 | 32,007 | 1.014 | 1.003 |
| string_functions | 32 | 77,389 | 81,116 | 1.048 | 1.041 |
| subquery_in | 1 | 880 | 894 | 1.016 | 0.998 |
| subquery_in | 32 | 1,215 | 1,283 | 1.055 | 1.035 |
| table_count | 1 | 2,366 | 2,382 | 1.007 | 1.029 |
| table_count | 32 | 28,585 | 18,285 | 0.640 | 1.068 |
| table_group_by | 1 | 21 | 22 | 1.048 | 1.015 |
| table_group_by | 32 | 40 | 45 | 1.120 | 1.049 |
| text_read | 1 | 599,401 | 605,433 | 1.010 | 1.007 |
| text_read | 32 | 828,461 | 805,931 | 0.973 | 0.983 |
| txn_read_write | 1 | 202 | 201 | 0.993 | 0.979 |
| txn_read_write | 32 | 3,897 | 3,915 | 1.005 | 0.994 |
| union_all | 1 | 581,367 | 588,043 | 1.011 | 1.003 |
| union_all | 32 | 840,936 | 818,716 | 0.974 | 0.977 |
| update_indexed | 1 | 194 | 208 | 1.072 | 0.990 |
| update_indexed | 32 | 5,491 | 6,521 | 1.187 | 0.993 |
| update_json | 1 | 198 | 198 | 1.000 | 0.979 |
| update_json | 32 | 6,336 | 6,673 | 1.053 | 0.997 |
| update_plain | 1 | 198 | 199 | 1.005 | 0.994 |
| update_plain | 32 | 6,317 | 7,430 | 1.176 | 0.974 |
| update_range | 1 | 197 | 193 | 0.980 | 0.937 |
| update_range | 32 | 5,568 | 5,613 | 1.008 | 1.044 |
| update_text | 1 | 198 | 199 | 1.005 | 0.991 |
| update_text | 32 | 6,355 | 6,299 | 0.991 | 1.013 |
| upsert | 1 | 197 | 201 | 1.019 | 0.957 |
| upsert | 32 | 6,086 | 6,354 | 1.044 | 0.992 |
| window_row_number | 1 | 1,458 | 1,487 | 1.020 | 1.002 |
| window_row_number | 32 | 8,088 | 8,510 | 1.052 | 1.031 |

## Reading

Of the 104 statement and client pairs, 94 are within 3% or faster by best of 4; ten are below
0.97. None of the ten runs a program the change altered, and each is within its own binary's spread
on this machine:

| pair | best ratio | before's four runs | after's four runs |
|---|---:|---|---|
| table_count, 32 | 0.640 | 1,691 to 28,585 | 6,690 to 18,285 |
| insert_batch_100, 32 | 0.925 | 262 to 643 | 364 to 595 |
| delete_range_insert, 1 | 0.929 | 185 to 209 | 161 to 194 |
| filter_is_null, 1 | 0.940 | 9,417 to 10,393 | 9,290 to 9,773 |
| point_text, 1 | 0.944 | 393,812 to 412,041 | 376,606 to 389,048 |
| insert_one, 32 | 0.952 | 4,365 to 6,573 | 4,248 to 6,257 |
| point_secondary, 1 | 0.956 | 417,803 to 452,139 | 421,610 to 432,033 |
| count_range, 1 | 0.965 | 2,708 to 3,574 | 2,819 to 3,447 |
| range_secondary_100, 32 | 0.968 | 841,743 to 870,930 | 808,324 to 842,792 |
| point_pk_one_column, 32 | 0.970 | 797,065 to 831,296 | 781,462 to 806,262 |

The point reads, the one group that looked consistently lower, were measured again with the
binaries alternated tightly (eight ABBA rounds, `--only point_text$,point_pk$,point_secondary$,text_read$`,
same machine, load under 2.5: a diagnosis, not a record): median ratios 0.984 to 1.004, and the
after binary slower in 3 to 6 of 8 adjacent pairs. `point_text` at 1 client: median 1.001, slower
in 4 of 8; at 32: 0.985. The 3.5% of the full run was the session's drift, not the binary.

What would show otherwise: a statement whose program differs between the binaries, or one whose
after runs fall below its before runs in most adjacent pairs of a tightly alternated run.

## The ten measured again

The same binaries on the same machine, a fresh database each run, each run started once the
1-minute load was under 2, before and after in ABBA order; the records are run.sh's fields with the
side, round and seconds waited for the load.

### The ten alone: `2026-10-26-adas-mac-studio-2-joined-set-rows-ten.jsonl`

2026-10-26, 23:11 to 23:51Z, `--only` the ten statements (both client counts), load at start 1.30
to 1.98. Six rounds were planned and four ran: before the fifth, the machine that runs kiln's server
rebooted, and the run ended with it. Best of 4:

| statement | clients | before best | after best | best ratio | median ratio | after slower in |
|---|---:|---:|---:|---:|---:|---:|
| count_range | 1 | 2,957 | 2,967 | 1.003 | 0.991 | 3 of 4 |
| count_range | 32 | 17,820 | 19,977 | 1.121 | 1.062 | 2 of 4 |
| delete_range_insert | 1 | 181 | 182 | 1.004 | 0.933 | 3 of 4 |
| delete_range_insert | 32 | 1,055 | 1,878 | 1.779 | 1.037 | 2 of 4 |
| filter_is_null | 1 | 10,708 | 10,782 | 1.007 | 1.018 | 1 of 4 |
| filter_is_null | 32 | 15,870 | 16,542 | 1.042 | 0.971 | 1 of 4 |
| insert_batch_100 | 1 | 138 | 133 | 0.962 | 0.917 | 4 of 4 |
| insert_batch_100 | 32 | 634 | 650 | 1.025 | 1.495 | 1 of 4 |
| insert_one | 1 | 179 | 186 | 1.035 | 0.986 | 3 of 4 |
| insert_one | 32 | 4,415 | 4,365 | 0.989 | 0.971 | 3 of 4 |
| point_pk_one_column | 1 | 647,767 | 636,625 | 0.983 | 0.975 | 4 of 4 |
| point_pk_one_column | 32 | 830,273 | 819,235 | 0.987 | 0.990 | 2 of 4 |
| point_secondary | 1 | 476,406 | 478,161 | 1.004 | 0.995 | 2 of 4 |
| point_secondary | 32 | 796,550 | 792,698 | 0.995 | 0.998 | 3 of 4 |
| point_text | 1 | 436,013 | 440,274 | 1.010 | 1.002 | 2 of 4 |
| point_text | 32 | 772,230 | 772,239 | 1.000 | 0.995 | 2 of 4 |
| range_secondary_100 | 1 | 262,730 | 261,282 | 0.994 | 0.994 | 4 of 4 |
| range_secondary_100 | 32 | 864,208 | 859,488 | 0.995 | 0.996 | 2 of 4 |
| table_count | 1 | 71,092 | 2,613 | 0.037 | 1.020 | 2 of 4 |
| table_count | 32 | 128,310 | 110,078 | 0.858 | 1.251 | 2 of 4 |

Nine of the ten are within 3% by best of 4. `table_count` is not, at either client count, for one
before run (round 3) that counted 71,092 times a second at a median of 0.013 ms where every other
run of either binary counted 2,375 to 2,613 times at 0.38 to 0.41 ms (without it, 1.058 at 1 client
and 1.016 at 32). `insert_batch_100` at 1 client, within 3% in the whole suite's run, is at 0.962
here; its two slowest after runs came one after the other, at 23:39 and 23:43Z, while the machine
that runs kiln's server went down.

### What those two compile to

`--query --explain` with both binaries on one database (a copy of the last run's):
`insert_batch_100`'s statement, one INSERT of 100 rows of 13 parameters, compiles to the same 1,478
lines of bytecode (SHA-256 6dd23ddf…f67f), and `table_count`'s, `SELECT COUNT(*) FROM items`, a
Count over the covering index `items_owner`, to the same program (9f27ae4c…700f).

### The two alone: `2026-10-27-adas-mac-studio-2-joined-set-rows-two.jsonl`

2026-10-27, 01:05 to 02:07Z, `--only 'table_count$,insert_batch_100$'`, six rounds, load at start
1.78 to 1.97. Best of 6:

| statement | clients | before best | after best | best ratio | median ratio | after slower in |
|---|---:|---:|---:|---:|---:|---:|
| insert_batch_100 | 1 | 137 | 139 | 1.015 | 0.974 | 4 of 6 |
| insert_batch_100 | 32 | 641 | 573 | 0.895 | 0.676 | 4 of 6 |
| table_count | 1 | 79,148 | 80,745 | 1.020 | 1.000 | 3 of 6 |
| table_count | 32 | 349,809 | 331,312 | 0.947 | 0.935 | 4 of 6 |

Here, with no other statement before it, `table_count` counts 76,086 to 80,745 times a second at 1
client on both binaries, as the one before run above did. At 32 clients the two spread wider than 3%
on either binary: `insert_batch_100` 381 to 641 statements a second across the before binary's six
runs, 348 to 573 across the after's.

### The two at 32 clients: `2026-10-27-adas-mac-studio-2-joined-set-rows-32.jsonl`

2026-10-27, 02:10 to 03:29Z, the same with `--clients 32`, load at start 1.81 to 1.99. Ten rounds
were planned and eight ran: the machine's kiln runner restarted while the ninth waited for the load,
and the rest was stopped. Best of 8:

| statement | clients | before best | after best | best ratio | median ratio | after slower in |
|---|---:|---:|---:|---:|---:|---:|
| insert_batch_100 | 32 | 625 | 671 | 1.074 | 1.002 | 4 of 8 |
| table_count | 32 | 300,800 | 333,847 | 1.110 | 1.013 | 2 of 8 |

## Reading, with the runs above

Each of the 104 pairs is within 3% by best of N in its longest run: 94 in the whole suite's (best
of 4), eight of the other ten in the ten's run (best of 4), and `insert_batch_100` and
`table_count` at 32 clients in their run of eight rounds (1.074 and 1.110); the same two at 1
client, which dipped in the ten's run, are within 3% in the run of the two (best of 6). The two at
32 clients move both ways from run to run, by best `insert_batch_100` 0.925, 1.025, 0.895 and 1.074
in the four runs and `table_count` 0.640, 0.858, 0.947 and 1.110, with medians of 1.002 and 1.013 in
the longest, and they compile to the same bytecode on either binary, prepared once, outside the
timed loop.
