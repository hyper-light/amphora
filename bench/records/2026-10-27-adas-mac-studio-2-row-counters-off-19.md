# The row counters off: the 19 statements outside 2% on jupiter, measured again on adas-mac-studio-2

Step 3 of the planner plan (kiln plan 01a16eab7f457673b4cf9933adacfc4d), its fourth criterion: the
suite's statements with the counters off within 2% of the change's parent, best of 3, on a quiet
machine (CLAUDE.md §7; `docs/design/planner.md`, decision 12). The jupiter record
(`2026-10-27-jupiter-row-counters-off-pairs.md`) left 21 pairs outside 2%, of 19 statements; each
of those statements was measured again here, at both client counts.

- **Builds:** amphora-bench in release from clean exports (`git archive`) of the jupiter record's
  commits: before, 335d5f4 (4a02b37, the change's parent, with dbef789's Windows build of the bench);
  after, 9b83cfc (016d245, the change, with the same). Their bench crates are the same; only the
  engine differs.
- **Machine:** adas-mac-studio-2: Apple M4 Max, 14 cores, 36 GiB, APFS, macOS 26.2 (a virtual machine
  holds about a quarter of a core).
- **Runs:** `--target amphora --suite --items 200000 --clients 1,32 --seconds 10 --warmup 2 --only
  <the 19 statements>`, six rounds in ABBA order (rounds 1, 3 and 5 before first), a fresh database
  each run, each run begun once the 1-minute load was under 2 (1.69 to 1.99), 2026-10-27 20:15 to
  21:59Z. The record, `2026-10-27-adas-mac-studio-2-row-counters-off-19.jsonl` (SHA-256
  925f3256…bc9c), has run.sh's fields with each run's side, round and seconds waited for the load.

## Round 1 left out

The session's first run (round 1's, the before build) wrote faster at every single-client write than
any of the other eleven runs of either build, the after build's first run among them: `insert_one`
at 1 client 230 statements a second against 174 to 183, `update_range` 195 against 163 to 176,
`delete_insert` 194 against 170 to 173, and `insert_one` at 32 clients 6,585 against 4,266 to 4,854:
the machine's first write burst of the session. Best of the other five rounds, each build's spread
its highest run over its lowest:

| statement | clients | before best | after best | best ratio | median ratio | before spread | after spread |
|---|---:|---:|---:|---:|---:|---:|---:|
| date_bucket | 1 | 374 | 391 | 1.046 | 1.046 | 1.019 | 1.018 |
| date_bucket | 32 | 876 | 894 | 1.020 | 1.001 | 1.144 | 1.068 |
| delete_insert | 1 | 173 | 173 | 1.001 | 0.990 | 1.013 | 1.016 |
| delete_insert | 32 | 4,274 | 4,285 | 1.002 | 1.027 | 1.083 | 1.101 |
| delete_range_insert | 1 | 178 | 180 | 1.010 | 1.031 | 1.049 | 1.105 |
| delete_range_insert | 32 | 1,016 | 1,035 | 1.019 | 1.029 | 1.056 | 1.125 |
| filter_multi_column | 1 | 1,704 | 1,672 | 0.981 | 0.994 | 1.039 | 1.020 |
| filter_multi_column | 32 | 2,082 | 2,132 | 1.024 | 0.999 | 1.136 | 1.070 |
| filter_no_index | 1 | 41 | 41 | 1.000 | 1.012 | 1.059 | 1.067 |
| filter_no_index | 32 | 44 | 45 | 1.014 | 0.998 | 1.151 | 1.077 |
| in_list_10 | 1 | 78,622 | 78,960 | 1.004 | 1.003 | 1.009 | 1.019 |
| in_list_10 | 32 | 122,959 | 125,916 | 1.024 | 1.004 | 1.128 | 1.061 |
| insert_batch_100 | 1 | 158 | 155 | 0.984 | 0.983 | 1.072 | 1.192 |
| insert_batch_100 | 32 | 329 | 329 | 1.001 | 0.824 | 1.574 | 1.604 |
| insert_one | 1 | 180 | 183 | 1.012 | 1.005 | 1.035 | 1.035 |
| insert_one | 32 | 4,854 | 4,643 | 0.956 | 0.971 | 1.090 | 1.088 |
| like_substring | 1 | 312 | 332 | 1.066 | 1.038 | 1.056 | 1.062 |
| like_substring | 32 | 599 | 610 | 1.017 | 0.992 | 1.134 | 1.069 |
| order_by_sort | 1 | 501 | 497 | 0.992 | 0.994 | 1.033 | 1.023 |
| order_by_sort | 32 | 625 | 642 | 1.027 | 0.995 | 1.134 | 1.070 |
| point_pk_one_column | 1 | 640,952 | 639,482 | 0.998 | 0.993 | 1.009 | 1.019 |
| point_pk_one_column | 32 | 812,275 | 845,288 | 1.041 | 1.026 | 1.023 | 1.070 |
| range_pk_100 | 1 | 63,132 | 63,678 | 1.009 | 0.998 | 1.010 | 1.027 |
| range_pk_100 | 32 | 81,328 | 83,502 | 1.027 | 1.002 | 1.136 | 1.068 |
| range_secondary_100 | 1 | 259,094 | 263,042 | 1.015 | 1.015 | 1.013 | 1.033 |
| range_secondary_100 | 32 | 851,728 | 872,069 | 1.024 | 1.025 | 1.012 | 1.051 |
| string_functions | 1 | 31,109 | 33,062 | 1.063 | 1.064 | 1.013 | 1.028 |
| string_functions | 32 | 82,630 | 84,324 | 1.021 | 0.997 | 1.143 | 1.069 |
| txn_read_write | 1 | 176 | 176 | 1.002 | 0.990 | 1.021 | 1.032 |
| txn_read_write | 32 | 3,829 | 3,847 | 1.005 | 1.003 | 1.043 | 1.027 |
| update_indexed | 1 | 175 | 175 | 1.002 | 1.007 | 1.026 | 1.026 |
| update_indexed | 32 | 5,151 | 5,231 | 1.015 | 0.985 | 1.058 | 1.083 |
| update_plain | 1 | 174 | 174 | 1.005 | 0.999 | 1.033 | 1.016 |
| update_plain | 32 | 5,830 | 5,796 | 0.994 | 0.966 | 1.193 | 1.115 |
| update_range | 1 | 172 | 176 | 1.023 | 1.003 | 1.021 | 1.081 |
| update_range | 32 | 5,234 | 5,194 | 0.992 | 0.981 | 1.037 | 1.060 |
| update_text | 1 | 179 | 172 | 0.960 | 0.985 | 1.057 | 1.024 |
| update_text | 32 | 5,811 | 5,734 | 0.987 | 0.970 | 1.114 | 1.117 |

## Reading

24 of the 38 pairs are within 2% by best of 5; 12 are faster with the counters off and 2 slower:
`insert_one` at 32 clients (0.956) and `update_text` at 1 (0.960), each inside its builds' own
spread (1.090 and 1.088; 1.057 and 1.024). Five pairs are faster with the counters off in every
round: `date_bucket`, `like_substring` and `string_functions` at 1 client (best 1.046, 1.066 and
1.063; medians 1.046, 1.038, 1.064) and `point_pk_one_column` and `range_secondary_100` at 32
(1.041 and 1.024; medians 1.026 and 1.025); jupiter found `string_functions` at 1 client faster too
(1.202). The change leaves its instructions out of a statement whose connection asked for no counts
(`crates/engine/tests/row_counts.rs`, `counters_are_off_unless_asked`), so the faster pairs are the
two binaries' other differences, not the counters' work.

As on jupiter, every pair within 2% either way by best of 3 isn't met: a single-client write varies
by more than 2% between runs of one binary here (`insert_batch_100` at 1 client: 1.072 and 1.192),
and five pairs are steadily faster with the counters off. On neither machine is a pair slower with
the counters off by more than its builds' own runs vary. Ada judged the criterion met on that
evidence (2026-10-27).

All six rounds, round 1 included (best of 6):

| statement | clients | before best | after best | best ratio | median ratio | before spread | after spread |
|---|---:|---:|---:|---:|---:|---:|---:|
| date_bucket | 1 | 374 | 391 | 1.046 | 1.046 | 1.038 | 1.018 |
| date_bucket | 32 | 876 | 894 | 1.020 | 0.991 | 1.144 | 1.152 |
| delete_insert | 1 | 194 | 173 | 0.891 | 0.990 | 1.138 | 1.019 |
| delete_insert | 32 | 4,274 | 4,285 | 1.002 | 1.022 | 1.090 | 1.101 |
| delete_range_insert | 1 | 178 | 180 | 1.011 | 1.042 | 1.086 | 1.105 |
| delete_range_insert | 32 | 1,016 | 1,035 | 1.019 | 1.037 | 1.122 | 1.125 |
| filter_multi_column | 1 | 1,704 | 1,672 | 0.981 | 0.997 | 1.232 | 1.020 |
| filter_multi_column | 32 | 2,087 | 2,132 | 1.022 | 0.981 | 1.139 | 1.146 |
| filter_no_index | 1 | 41 | 42 | 1.015 | 1.024 | 1.087 | 1.083 |
| filter_no_index | 32 | 44 | 45 | 1.014 | 0.990 | 1.151 | 1.167 |
| in_list_10 | 1 | 78,622 | 78,960 | 1.004 | 1.001 | 1.009 | 1.019 |
| in_list_10 | 32 | 122,959 | 125,916 | 1.024 | 0.993 | 1.128 | 1.136 |
| insert_batch_100 | 1 | 158 | 155 | 0.984 | 0.982 | 1.805 | 1.395 |
| insert_batch_100 | 32 | 329 | 329 | 1.001 | 0.911 | 1.694 | 1.604 |
| insert_one | 1 | 230 | 183 | 0.795 | 0.995 | 1.317 | 1.040 |
| insert_one | 32 | 6,585 | 4,643 | 0.705 | 0.960 | 1.478 | 1.088 |
| like_substring | 1 | 312 | 332 | 1.066 | 1.059 | 1.056 | 1.062 |
| like_substring | 32 | 599 | 610 | 1.017 | 0.984 | 1.134 | 1.136 |
| order_by_sort | 1 | 501 | 497 | 0.992 | 0.996 | 1.061 | 1.027 |
| order_by_sort | 32 | 625 | 642 | 1.027 | 0.986 | 1.134 | 1.148 |
| point_pk_one_column | 1 | 640,952 | 639,482 | 0.998 | 0.990 | 1.009 | 1.019 |
| point_pk_one_column | 32 | 812,275 | 845,288 | 1.041 | 1.026 | 1.023 | 1.070 |
| range_pk_100 | 1 | 63,132 | 63,678 | 1.009 | 0.995 | 1.210 | 1.027 |
| range_pk_100 | 32 | 81,328 | 83,502 | 1.027 | 0.990 | 1.136 | 1.152 |
| range_secondary_100 | 1 | 259,094 | 263,042 | 1.015 | 1.013 | 1.104 | 1.033 |
| range_secondary_100 | 32 | 851,728 | 872,069 | 1.024 | 1.022 | 1.029 | 1.051 |
| string_functions | 1 | 31,109 | 33,095 | 1.064 | 1.066 | 1.028 | 1.029 |
| string_functions | 32 | 82,630 | 84,324 | 1.021 | 0.987 | 1.143 | 1.155 |
| txn_read_write | 1 | 183 | 176 | 0.961 | 0.988 | 1.064 | 1.032 |
| txn_read_write | 32 | 3,829 | 3,847 | 1.005 | 1.012 | 1.187 | 1.027 |
| update_indexed | 1 | 179 | 175 | 0.979 | 0.995 | 1.050 | 1.026 |
| update_indexed | 32 | 5,151 | 5,231 | 1.015 | 0.979 | 1.140 | 1.083 |
| update_plain | 1 | 182 | 174 | 0.961 | 0.997 | 1.079 | 1.016 |
| update_plain | 32 | 5,830 | 5,796 | 0.994 | 0.985 | 1.193 | 1.134 |
| update_range | 1 | 195 | 176 | 0.903 | 0.994 | 1.156 | 1.081 |
| update_range | 32 | 5,234 | 5,194 | 0.992 | 0.982 | 1.037 | 1.060 |
| update_text | 1 | 189 | 172 | 0.909 | 0.981 | 1.116 | 1.024 |
| update_text | 32 | 5,952 | 5,734 | 0.963 | 0.952 | 1.141 | 1.121 |
