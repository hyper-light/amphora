# The row counters off: the bench suite before and after, in turns

Step 3 of the planner plan (kiln plan 01a16eab7f457673b4cf9933adacfc4d; CLAUDE.md §7;
docs/design/planner.md, decision 12): with no connection asking for row counts, the suite's
statements run as fast as before the change.

- **Builds:** amphora-bench in release from clean checkouts (`dirty_files` 0 in every record):
  before, 4a02b37 (the change's parent); after, 016d245 (the change). Their bench crates are the
  same; only the engine differs.
- **Machine:** adas-mac-studio-2: Apple M4 Max, 14 cores, 36 GiB, APPLE SSD AP0512Z (APFS),
  macOS 26.2 (Darwin 25.2.0), the machine Ada named for the suite's before-and-after runs on
  2026-10-25. fedora was offline and jupiter too noisy (below).
- **Runs:** `bench/planner/suite-turns.sh` with `--target amphora --items 200000 --clients 1,32
  --seconds 5 --warmup 1`, before and after in turns, three each, 2026-10-26 08:58 to 10:38Z, each
  begun at a one-minute load under 2 (1.89 to 1.96). Other work came onto the machine during the
  runs (loads after them 3.85 to 9.59): both builds' runs had it.
- **Compared:** `python3 bench/planner/suite_compare.py`, each statement's highest throughput of
  its build's three runs at each client count: 52 statements at 1 and 32 clients, 104 pairs.

## The result: `2026-10-26-adas-mac-studio-2-row-counters-off.jsonl`

After over before: **median 0.9987, geometric mean 1.0161** over the 104 pairs; 48 of them within
2%, the lowest 0.862 (`like_substring`, 1 client) and the highest 1.809 (`delete_range_insert`, 32
clients). The pairs that differ more than 2% go both ways (25 below, 31 above), and a build's own
three runs differ by more (the highest over the lowest of each pair's three: median 1.073 before,
1.083 after; 90th percentile 1.224 and 1.645), so a single statement's difference is within what
the machine varies; the whole suite's is within 2%. That is what the change should give: a
statement prepared on a connection that didn't ask for counts has no counting instruction
(`crates/engine/tests/row_counts.rs`, `counters_are_off_unless_asked`), and its planning differs
only by a check of an empty option where each query opens its loops.

| statement | clients | before /s | after /s | after/before |
|---|---:|---:|---:|---:|
| aggregates_range | 1 | 5827.8 | 5497.8 | 0.9434 |
| aggregates_range | 32 | 8090.2 | 7974.8 | 0.9857 |
| blob_read | 1 | 606148.8 | 604033.0 | 0.9965 |
| blob_read | 32 | 806512.2 | 797392.8 | 0.9887 |
| case_aggregate | 1 | 5607.4 | 5733.8 | 1.0225 |
| case_aggregate | 32 | 7910.0 | 7470.2 | 0.9444 |
| count_distinct | 1 | 460.4 | 463.0 | 1.0056 |
| count_distinct | 32 | 599.8 | 584.6 | 0.9747 |
| count_range | 1 | 2959.2 | 3494.4 | 1.1809 |
| count_range | 32 | 16720.6 | 18748.4 | 1.1213 |
| cte | 1 | 370.2 | 367.8 | 0.9935 |
| cte | 32 | 595.6 | 563.8 | 0.9466 |
| date_bucket | 1 | 355.0 | 370.8 | 1.0445 |
| date_bucket | 32 | 833.2 | 788.2 | 0.9460 |
| delete_insert | 1 | 184.0 | 204.8 | 1.1130 |
| delete_insert | 32 | 4198.4 | 6578.6 | 1.5669 |
| delete_range_insert | 1 | 173.2 | 192.8 | 1.1132 |
| delete_range_insert | 32 | 1099.6 | 1989.0 | 1.8088 |
| distinct_range | 1 | 621.6 | 617.6 | 0.9936 |
| distinct_range | 32 | 810.8 | 795.4 | 0.9810 |
| exists_subquery | 1 | 532.2 | 534.2 | 1.0038 |
| exists_subquery | 32 | 730.0 | 719.0 | 0.9849 |
| filter_is_null | 1 | 9670.2 | 9544.2 | 0.9870 |
| filter_is_null | 32 | 13570.2 | 12678.2 | 0.9343 |
| filter_multi_column | 1 | 1600.4 | 1649.0 | 1.0304 |
| filter_multi_column | 32 | 1918.2 | 1772.0 | 0.9238 |
| filter_no_index | 1 | 38.0 | 38.8 | 1.0211 |
| filter_no_index | 32 | 40.4 | 38.4 | 0.9505 |
| group_by_having | 1 | 400.2 | 414.6 | 1.0360 |
| group_by_having | 32 | 592.4 | 577.8 | 0.9754 |
| group_by_range | 1 | 469.8 | 466.2 | 0.9923 |
| group_by_range | 32 | 801.2 | 791.8 | 0.9883 |
| group_correlated | 1 | 25654.4 | 25143.0 | 0.9801 |
| group_correlated | 32 | 229036.0 | 231600.4 | 1.0112 |
| group_correlated_row | 1 | 141608.8 | 130742.8 | 0.9233 |
| group_correlated_row | 32 | 786289.6 | 783090.0 | 0.9959 |
| in_list_10 | 1 | 75077.0 | 77640.8 | 1.0341 |
| in_list_10 | 32 | 114542.2 | 107446.2 | 0.9380 |
| insert_batch_100 | 1 | 167.4 | 160.6 | 0.9594 |
| insert_batch_100 | 32 | 402.4 | 360.2 | 0.8951 |
| insert_one | 1 | 198.8 | 222.0 | 1.1167 |
| insert_one | 32 | 5867.6 | 6719.6 | 1.1452 |
| join_group | 1 | 83.6 | 84.2 | 1.0072 |
| join_group | 32 | 116.8 | 114.8 | 0.9829 |
| join_pk | 1 | 128756.0 | 125499.8 | 0.9747 |
| join_pk | 32 | 312292.6 | 296230.2 | 0.9486 |
| join_three | 1 | 2428.4 | 2428.2 | 0.9999 |
| join_three | 32 | 8478.8 | 7975.2 | 0.9406 |
| json_field | 1 | 476853.8 | 480778.2 | 1.0082 |
| json_field | 32 | 807882.2 | 800061.2 | 0.9903 |
| json_filter | 1 | 404.0 | 400.2 | 0.9906 |
| json_filter | 32 | 660.4 | 629.4 | 0.9531 |
| left_join_anti | 1 | 589.4 | 589.8 | 1.0007 |
| left_join_anti | 32 | 714.0 | 706.4 | 0.9894 |
| like_prefix | 1 | 211417.2 | 233689.6 | 1.1053 |
| like_prefix | 32 | 839239.8 | 828112.4 | 0.9867 |
| like_substring | 1 | 288.4 | 248.6 | 0.8620 |
| like_substring | 32 | 425.4 | 400.6 | 0.9417 |
| offset_deep | 1 | 92.8 | 94.4 | 1.0172 |
| offset_deep | 32 | 82.0 | 81.0 | 0.9878 |
| order_by_index | 1 | 201431.4 | 201179.2 | 0.9987 |
| order_by_index | 32 | 775749.2 | 779283.6 | 1.0046 |
| order_by_sort | 1 | 470.0 | 459.4 | 0.9774 |
| order_by_sort | 32 | 589.2 | 582.0 | 0.9878 |
| point_pk | 1 | 574414.4 | 564959.2 | 0.9835 |
| point_pk | 32 | 801924.8 | 814101.6 | 1.0152 |
| point_pk_one_column | 1 | 645316.2 | 659275.6 | 1.0216 |
| point_pk_one_column | 32 | 809265.0 | 820879.8 | 1.0144 |
| point_secondary | 1 | 469135.6 | 468418.8 | 0.9985 |
| point_secondary | 32 | 793600.2 | 796806.2 | 1.0040 |
| point_text | 1 | 424719.4 | 428906.4 | 1.0099 |
| point_text | 32 | 766286.0 | 766682.0 | 1.0005 |
| range_pk_100 | 1 | 59267.2 | 60234.2 | 1.0163 |
| range_pk_100 | 32 | 75626.8 | 71072.2 | 0.9398 |
| range_secondary_100 | 1 | 250947.4 | 254151.6 | 1.0128 |
| range_secondary_100 | 32 | 852116.4 | 871322.8 | 1.0225 |
| string_functions | 1 | 29766.8 | 31600.6 | 1.0616 |
| string_functions | 32 | 76808.8 | 73417.8 | 0.9559 |
| subquery_in | 1 | 880.6 | 885.4 | 1.0055 |
| subquery_in | 32 | 1206.8 | 1183.6 | 0.9808 |
| table_count | 1 | 2469.8 | 2392.2 | 0.9686 |
| table_count | 32 | 20176.0 | 21181.4 | 1.0498 |
| table_group_by | 1 | 21.6 | 21.6 | 1.0000 |
| table_group_by | 32 | 39.8 | 38.4 | 0.9648 |
| text_read | 1 | 596525.6 | 591609.4 | 0.9918 |
| text_read | 32 | 804794.2 | 798702.8 | 0.9924 |
| txn_read_write | 1 | 177.8 | 194.8 | 1.0956 |
| txn_read_write | 32 | 3737.2 | 3774.2 | 1.0099 |
| union_all | 1 | 573137.6 | 574565.8 | 1.0025 |
| union_all | 32 | 820400.6 | 813849.8 | 0.9920 |
| update_indexed | 1 | 189.8 | 196.8 | 1.0369 |
| update_indexed | 32 | 4915.2 | 5088.0 | 1.0352 |
| update_json | 1 | 176.6 | 190.8 | 1.0804 |
| update_json | 32 | 5657.6 | 6246.4 | 1.1041 |
| update_plain | 1 | 180.6 | 180.0 | 0.9967 |
| update_plain | 32 | 5977.6 | 6233.6 | 1.0428 |
| update_range | 1 | 181.4 | 197.4 | 1.0882 |
| update_range | 32 | 5171.2 | 7123.2 | 1.3775 |
| update_text | 1 | 187.4 | 195.4 | 1.0427 |
| update_text | 32 | 6003.2 | 6368.0 | 1.0608 |
| upsert | 1 | 190.6 | 196.0 | 1.0283 |
| upsert | 32 | 5798.4 | 6265.6 | 1.0806 |
| window_row_number | 1 | 1474.8 | 1457.8 | 0.9885 |
| window_row_number | 32 | 8181.0 | 7842.8 | 0.9587 |

## The first attempt: `2026-10-26-jupiter-row-counters-off.jsonl`

The same comparison on jupiter (AMD Ryzen AI Max+ 395, 32 threads, 124 GiB, WD PC SN5000S, NTFS,
Windows 11 Pro 10.0.26200), 2026-10-26 04:17 to 05:44Z, with a bench patched to build on Windows
(335d5f4 before, 9b83cfc after: 4a02b37 and 016d245 each with dbef789's change that builds the
bench on Windows and nothing else, on local branches `t1/rowcounts-before` and
`t1/rowcounts-after-suite`, not for merge). A build's own runs there differed by a median of 15% and up
to 19 times (`table_count` at 32 clients), so it can't tell 2%; it is kept as what was run, not as
evidence. A turn lost when the runner's connection closed was run again as turn 3.

## Measured again

The 56 pairs outside 2% above, each statement alone in ABBA turns on jupiter, 2026-10-27:
`2026-10-27-jupiter-row-counters-off-pairs.md` (35 of them within 2%; the 7 of the rest that are
slower with the counters off within their builds' own run-to-run variation).
