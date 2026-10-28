# The row counters off: the pairs outside 2%, each statement measured again alone

Step 3 of the planner plan (kiln plan 01a16eab7f457673b4cf9933adacfc4d, its fourth criterion:
the suite's statements with the counters off within 2% of the change's parent, best of 3, on a quiet
machine; CLAUDE.md §7; docs/design/planner.md, decision 12). 9cfc8b9's record
(`2026-10-26-adas-mac-studio-2-row-counters-off.md`) has its median and geometric mean within 2%, but
56 of its 104 statement and client-count pairs outside it by best of 3: 36 statements, 20 of them at
both client counts. Each of those pairs was measured again here.

- **Builds:** the same as that record's jupiter attempt: amphora-bench in release from clean checkouts
  (`dirty_files` 0 in every record): before, 335d5f4 (4a02b37, the change's parent, with dbef789's change
  that builds the bench on Windows); after, 9b83cfc (016d245, the change, with the same). Their bench
  crates are the same; only the engine differs.
- **Machine:** jupiter: AMD Ryzen AI Max+ 395 (16 cores, 32 threads), 123.6 GiB, WD PC SN5000S 1 TB
  NVMe (NTFS), Windows 11 Pro 10.0.26200.
- **Runs:** each statement alone (`--only '<statement>$'`), at the client counts its pairs need,
  with `--target amphora --items 200000 --seconds 10 --warmup 2`: before and after in three turns in
  ABBA order (`bench/planner/suite-turns.sh` at ddb321c: before, after; after, before; before,
  after), the six runs of a statement within minutes of each other, so a drift in the machine's
  speed falls on both builds alike. Each run began once the load was under 2 (Windows keeps no load
  average: the cores busy plus the processor queue, over 5 s), waiting up to an hour. 2026-10-27,
  03:36 to 06:07Z; a run the kiln runner's restart at 05:03 cut short was run again, and the turn it
  left half done got only the build it lacked.
- **Compared:** `python3 bench/planner/suite_compare.py <record> before after`: each pair's highest
  throughput of its build's three runs.

## The result: `2026-10-27-jupiter-row-counters-off-pairs.jsonl`

All 216 runs began at a load under 2 (0.32 to 1.97), three for each build at each pair. Other work
came onto the machine during 35 of them (a Rust build: loads after the run up to 28.2); a run it
slowed is one of three, and the best of the three is compared.

**35 of the 56 pairs are within 2%** (after over before, best of 3); over all 56 the median is
1.0063 (`suite_compare.py`'s) and the geometric mean 1.0085, and the build with the counters off was
the faster at 37. The other 21 go both ways: 14 faster with the counters off and 7 slower. Each
build's own three runs at a pair differ by more than 2% on this machine (the highest over the
lowest: median 1.050 before, 1.061 after; 90th percentile 1.274 and 1.391), and at 18 of the 21 the
two builds' runs overlap. The 7 slower are among those 18: no pair is slower with the counters off
by more than its own runs vary. The 3 that do not overlap are all faster with the counters off:
`insert_one` at 32 clients (1.090), `string_functions` at 1 (1.202) and `update_indexed` at 1
(1.030). The change leaves its instructions out of a statement whose connection didn't ask for
counts (`crates/engine/tests/row_counts.rs`, `counters_are_off_unless_asked`), so its own work can't
make them faster; what else differs between the two builds' binaries is not tested here.

So the criterion as written, every pair within 2% by best of 3, is not met on jupiter: 21 of the 56
pairs are outside it, 14 faster with the counters off and 7 slower. No pair is slower with the
counters off by more than its builds' own runs vary.

The two builds' runs at the 21 pairs outside 2%, lowest to highest:

| statement | clients | before /s, three runs | after /s, three runs | after/before | runs overlap |
|---|---:|---|---|---:|---|
| date_bucket | 32 | 291.7 to 297.4 | 291.8 to 308.3 | 1.0367 | yes |
| delete_insert | 32 | 1713.1 to 2496.2 | 1478.8 to 2414.6 | 0.9673 | yes |
| delete_range_insert | 32 | 446.0 to 873.5 | 462.0 to 898.7 | 1.0288 | yes |
| filter_multi_column | 32 | 524.1 to 539.3 | 526.4 to 550.8 | 1.0213 | yes |
| filter_no_index | 32 | 13.6 to 13.7 | 13.7 to 14.0 | 1.0219 | yes |
| in_list_10 | 32 | 20318.3 to 21058.1 | 20559.6 to 21727.3 | 1.0318 | yes |
| insert_batch_100 | 1 | 29.7 to 50.7 | 29.9 to 41.6 | 0.8205 | yes |
| insert_one | 32 | 2373.0 to 2557.4 | 2587.5 to 2787.2 | 1.0899 | no |
| like_substring | 32 | 87.8 to 95.5 | 86.8 to 91.2 | 0.9550 | yes |
| order_by_sort | 1 | 52.7 to 54.1 | 52.7 to 58.0 | 1.0721 | yes |
| point_pk_one_column | 1 | 83813.9 to 85895.6 | 77563.2 to 90779.9 | 1.0569 | yes |
| range_pk_100 | 32 | 27281.8 to 29448.6 | 27537.8 to 28428.5 | 0.9654 | yes |
| range_secondary_100 | 32 | 589848.8 to 619793.9 | 592281.8 to 604585.3 | 0.9755 | yes |
| string_functions | 1 | 5402.1 to 5693.9 | 5707.2 to 6845.7 | 1.2023 | no |
| string_functions | 32 | 25225.8 to 27224.2 | 24995.6 to 25633.5 | 0.9416 | yes |
| txn_read_write | 1 | 55.9 to 62.9 | 59.3 to 64.3 | 1.0223 | yes |
| update_indexed | 1 | 59.5 to 62.4 | 63.1 to 64.3 | 1.0304 | no |
| update_indexed | 32 | 1388.8 to 2019.4 | 1770.0 to 2446.9 | 1.2117 | yes |
| update_plain | 32 | 1957.2 to 2057.6 | 1844.7 to 2099.2 | 1.0202 | yes |
| update_range | 32 | 1614.2 to 1810.4 | 1196.8 to 1712.7 | 0.9460 | yes |
| update_text | 32 | 1827.4 to 1892.4 | 1841.2 to 1943.5 | 1.0270 | yes |

Every pair, best of 3 (`suite_compare.py`):

| statement | clients | before /s | after /s | after/before |
|---|---:|---:|---:|---:|
| aggregates_range | 1 | 1024.4 | 1018.3 | 0.9940 |
| case_aggregate | 1 | 1034.2 | 1029.9 | 0.9958 |
| case_aggregate | 32 | 3071.8 | 3096.8 | 1.0081 |
| count_distinct | 32 | 165.0 | 166.9 | 1.0115 |
| count_range | 1 | 1690.6 | 1708.5 | 1.0106 |
| count_range | 32 | 26522.8 | 26891.6 | 1.0139 |
| cte | 32 | 163.8 | 164.5 | 1.0043 |
| date_bucket | 1 | 82.4 | 82.8 | 1.0049 |
| date_bucket | 32 | 297.4 | 308.3 | 1.0367 |
| delete_insert | 1 | 62.7 | 63.1 | 1.0064 |
| delete_insert | 32 | 2496.2 | 2414.6 | 0.9673 |
| delete_range_insert | 1 | 62.8 | 62.7 | 0.9984 |
| delete_range_insert | 32 | 873.5 | 898.7 | 1.0288 |
| filter_is_null | 32 | 4369.3 | 4387.8 | 1.0042 |
| filter_multi_column | 1 | 241.0 | 243.8 | 1.0116 |
| filter_multi_column | 32 | 539.3 | 550.8 | 1.0213 |
| filter_no_index | 1 | 5.3 | 5.2 | 0.9811 |
| filter_no_index | 32 | 13.7 | 14.0 | 1.0219 |
| group_by_having | 1 | 63.3 | 63.7 | 1.0063 |
| group_by_having | 32 | 160.8 | 162.9 | 1.0131 |
| group_correlated_row | 1 | 69495.9 | 69863.7 | 1.0053 |
| in_list_10 | 1 | 10986.0 | 11139.9 | 1.0140 |
| in_list_10 | 32 | 21058.1 | 21727.3 | 1.0318 |
| insert_batch_100 | 1 | 50.7 | 41.6 | 0.8205 |
| insert_batch_100 | 32 | 110.5 | 110.3 | 0.9982 |
| insert_one | 1 | 64.6 | 64.7 | 1.0015 |
| insert_one | 32 | 2557.4 | 2787.2 | 1.0899 |
| join_pk | 1 | 27425.1 | 27565.0 | 1.0051 |
| join_pk | 32 | 82044.6 | 81807.1 | 0.9971 |
| join_three | 32 | 2985.5 | 3008.9 | 1.0078 |
| json_filter | 32 | 183.3 | 185.9 | 1.0142 |
| like_prefix | 1 | 100342.2 | 101728.5 | 1.0138 |
| like_substring | 1 | 43.1 | 42.9 | 0.9954 |
| like_substring | 32 | 95.5 | 91.2 | 0.9550 |
| order_by_sort | 1 | 54.1 | 58.0 | 1.0721 |
| point_pk_one_column | 1 | 85895.6 | 90779.9 | 1.0569 |
| range_pk_100 | 32 | 29448.6 | 28428.5 | 0.9654 |
| range_secondary_100 | 32 | 619793.9 | 604585.3 | 0.9755 |
| string_functions | 1 | 5693.9 | 6845.7 | 1.2023 |
| string_functions | 32 | 27224.2 | 25633.5 | 0.9416 |
| table_count | 1 | 26188.7 | 26206.8 | 1.0007 |
| table_count | 32 | 290126.4 | 288668.4 | 0.9950 |
| table_group_by | 32 | 15.0 | 15.1 | 1.0067 |
| txn_read_write | 1 | 62.9 | 64.3 | 1.0223 |
| update_indexed | 1 | 62.4 | 64.3 | 1.0304 |
| update_indexed | 32 | 2019.4 | 2446.9 | 1.2117 |
| update_json | 1 | 64.8 | 64.1 | 0.9892 |
| update_json | 32 | 2157.4 | 2167.2 | 1.0045 |
| update_plain | 32 | 2057.6 | 2099.2 | 1.0202 |
| update_range | 1 | 63.2 | 62.3 | 0.9858 |
| update_range | 32 | 1810.4 | 1712.7 | 0.9460 |
| update_text | 1 | 61.6 | 62.0 | 1.0065 |
| update_text | 32 | 1892.4 | 1943.5 | 1.0270 |
| upsert | 1 | 63.0 | 63.1 | 1.0016 |
| upsert | 32 | 2139.0 | 2129.7 | 0.9957 |
| window_row_number | 32 | 3386.6 | 3383.9 | 0.9992 |

## Measured again

The 19 statements behind the 21 pairs outside 2% here, each at both client counts in six ABBA rounds
on adas-mac-studio-2, 2026-10-27: `2026-10-27-adas-mac-studio-2-row-counters-off-19.md`. On neither
machine is a pair slower with the counters off by more than its builds' own runs vary; Ada judged
the criterion met on that evidence (2026-10-27).
