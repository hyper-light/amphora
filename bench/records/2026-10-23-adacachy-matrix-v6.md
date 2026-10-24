# Matrix: best of the runs given

- host: adacachy, AMD Ryzen 9 9950X3D2 16-Core Processor (32 threads), disk Samsung SSD 9100 PRO 2TB (btrfs), kernel 7.2.9-2-cachyos
- runs: 48, 2026-10-23T09:41:26Z to 2026-10-23T23:36:22Z
- load average around the runs: 3.5 to 123.6 (not a quiet machine: CLAUDE.md §7 asks under 2)
- commits: 75fbf1f365 +16 dirty, 75fbf1f365 +28 dirty

## Suite: best of each cell, losses first

passes per target: amphora 3, cedardb 3, clickhouse 3, mysql 3, orioledb 3, postgres 3, sqlite 3, turso 3

99 cells: Amphora first in 19, ahead of every row store in 37.

| statement | clients | Amphora /s | Amphora p99 ms | place | best other | its /s | Amphora / best | Amphora / best row store |
|---|---:|---:|---:|---|---|---:|---:|---:|
| order_by_sort | 1 | 0.4 | 3211.264 | 7/8 | cedardb | 2,397 | 0.000 | 0.04 |
| filter_no_index | 1 | 0.6 | 1589.248 | 7/8 | cedardb | 2,608 | 0.000 | 0.23 |
| table_group_by | 1 | 0.2 | 4784.128 | 4/4 | cedardb | 276 | 0.001 | 0.12 |
| filter_no_index | 32 | 6.4 | 6881.280 | 3/6 | cedardb | 7,541 | 0.001 | 1.00 |
| exists_subquery | 1 | 3.0 | 2981.888 | 6/6 | cedardb | 2,051 | 0.001 | 0.21 |
| exists_subquery | 32 | 35.6 | 6619.136 | 4/7 | cedardb | 6,313 | 0.006 | 0.50 |
| join_group | 1 | 0.8 | 1359.872 | 5/6 | cedardb | 135 | 0.006 | 0.50 |
| cte | 1 | 2.4 | 471.040 | 6/7 | cedardb | 401 | 0.006 | 0.27 |
| order_by_sort | 32 | 52.6 | 2228.224 | 3/6 | cedardb | 8,251 | 0.006 | 1.58 |
| group_by_having | 1 | 4.8 | 223.232 | 6/7 | cedardb | 507 | 0.009 | 0.52 |
| table_count | 32 | 140 | 483.328 | 3/7 | clickhouse | 14,127 | 0.010 | 1.12 |
| count_distinct | 1 | 7.2 | 202.752 | 3/7 | cedardb | 570 | 0.013 | 1.00 |
| table_count | 1 | 36.8 | 32.512 | 5/7 | cedardb | 2,197 | 0.017 | 0.36 |
| offset_deep | 1 | 0.4 | 2850.816 | 7/7 | clickhouse | 15.2 | 0.026 | 0.22 |
| count_distinct | 32 | 52.8 | 1032.192 | 3/7 | cedardb | 1,002 | 0.053 | 1.43 |
| cte | 32 | 28.0 | 1736.704 | 4/6 | cedardb | 519 | 0.054 | 0.74 |
| offset_deep | 32 | 7.2 | 6881.280 | 2/6 | clickhouse | 114 | 0.063 | 1.38 |
| join_group | 32 | 11.0 | 4653.056 | 3/5 | cedardb | 173 | 0.064 | 5.00 |
| count_range | 1 | 1,618 | 0.944 | 3/7 | cedardb | 20,632 | 0.078 | 0.47 |
| group_by_having | 32 | 42.2 | 1261.568 | 3/7 | cedardb | 447 | 0.094 | 1.07 |
| subquery_in | 1 | 174 | 7.424 | 3/7 | cedardb | 1,615 | 0.108 | 0.25 |
| count_range | 32 | 22,488 | 4.416 | 2/7 | cedardb | 129,952 | 0.173 | 8.55 |
| insert_batch_100 | 32 | 54.0 | 974.848 | 6/7 | cedardb | 312 | 0.173 | 0.71 |
| json_filter | 1 | 182 | 12.160 | 5/7 | cedardb | 978 | 0.186 | 0.28 |
| case_aggregate | 1 | 1,398 | 1.984 | 4/7 | sqlite | 7,288 | 0.192 | 0.19 |
| left_join_anti | 1 | 282 | 6.144 | 5/7 | cedardb | 1,322 | 0.213 | 0.94 |
| date_bucket | 1 | 197 | 7.424 | 5/7 | cedardb | 695 | 0.284 | 0.45 |
| filter_multi_column | 1 | 684 | 18.176 | 4/8 | cedardb | 2,243 | 0.305 | 0.97 |
| blob_read | 1 | 161,828 | 0.010 | 3/7 | sqlite | 514,818 | 0.314 | 0.31 |
| date_bucket | 32 | 2,294 | 26.112 | 4/7 | clickhouse | 7,133 | 0.322 | 0.73 |
| subquery_in | 32 | 2,374 | 29.184 | 3/7 | postgres | 6,504 | 0.365 | 0.36 |
| json_field | 1 | 164,954 | 0.010 | 3/7 | sqlite | 443,395 | 0.372 | 0.37 |
| union_all | 1 | 69,236 | 0.127 | 3/7 | turso | 169,623 | 0.408 | 0.41 |
| left_join_anti | 32 | 3,417 | 23.808 | 3/7 | clickhouse | 8,366 | 0.409 | 2.14 |
| update_json | 1 | 148 | 68.608 | 4/6 | sqlite | 355 | 0.418 | 0.42 |
| text_read | 1 | 213,784 | 0.007 | 3/7 | sqlite | 501,781 | 0.426 | 0.43 |
| group_by_range | 1 | 346 | 4.032 | 5/7 | cedardb | 811 | 0.427 | 0.83 |
| window_row_number | 1 | 887 | 1.600 | 3/7 | sqlite | 2,041 | 0.434 | 0.43 |
| like_substring | 1 | 261 | 6.464 | 2/8 | sqlite | 554 | 0.471 | 0.47 |
| delete_range_insert | 32 | 424 | 256.000 | 3/6 | cedardb | 894 | 0.474 | 0.93 |
| filter_is_null | 32 | 127,703 | 1.824 | 2/8 | postgres | 231,694 | 0.551 | 0.55 |
| point_text | 32 | 1,062,052 | 0.140 | 2/8 | cedardb | 1,917,239 | 0.554 | 1.40 |
| group_by_range | 32 | 3,643 | 22.528 | 3/7 | clickhouse | 6,572 | 0.554 | 1.06 |
| text_read | 32 | 927,973 | 0.544 | 3/7 | cedardb | 1,623,963 | 0.571 | 0.97 |
| insert_one | 1 | 179 | 25.344 | 3/7 | sqlite | 312 | 0.574 | 0.57 |
| string_functions | 32 | 224,018 | 1.744 | 3/7 | cedardb | 375,680 | 0.596 | 0.94 |
| delete_insert | 1 | 209 | 11.392 | 3/6 | turso | 340 | 0.615 | 0.61 |
| point_pk_one_column | 1 | 302,040 | 0.005 | 3/8 | sqlite | 488,349 | 0.618 | 0.62 |
| point_secondary | 32 | 1,039,448 | 0.154 | 2/8 | cedardb | 1,679,664 | 0.619 | 1.43 |
| json_field | 32 | 1,068,653 | 0.528 | 2/7 | cedardb | 1,725,823 | 0.619 | 1.04 |
| delete_range_insert | 1 | 138 | 24.064 | 3/6 | sqlite | 215 | 0.640 | 0.64 |
| update_plain | 1 | 230 | 18.176 | 3/6 | sqlite | 347 | 0.662 | 0.66 |
| window_row_number | 32 | 11,984 | 7.808 | 3/7 | postgres | 17,876 | 0.670 | 0.67 |
| distinct_range | 1 | 463 | 3.808 | 5/7 | cedardb | 688 | 0.673 | 0.83 |
| order_by_index | 32 | 1,070,016 | 0.272 | 2/8 | turso | 1,572,266 | 0.681 | 0.68 |
| union_all | 32 | 904,461 | 0.328 | 3/7 | cedardb | 1,328,576 | 0.681 | 0.89 |
| distinct_range | 32 | 4,692 | 17.408 | 3/7 | clickhouse | 6,869 | 0.683 | 1.61 |
| in_list_10 | 1 | 49,963 | 0.028 | 2/8 | sqlite | 72,853 | 0.686 | 0.69 |
| join_pk | 32 | 517,454 | 0.856 | 2/7 | cedardb | 725,288 | 0.713 | 4.71 |
| insert_batch_100 | 1 | 57.8 | 67.584 | 3/7 | cedardb | 80.0 | 0.722 | 1.00 |
| point_pk_one_column | 32 | 1,258,514 | 0.472 | 2/8 | cedardb | 1,722,933 | 0.730 | 1.28 |
| txn_read_write | 1 | 250 | 8.704 | 3/6 | sqlite | 336 | 0.745 | 0.74 |
| upsert | 1 | 258 | 11.008 | 3/6 | sqlite | 344 | 0.751 | 0.75 |
| insert_one | 32 | 2,361 | 137.216 | 2/7 | postgres | 3,121 | 0.756 | 0.76 |
| range_pk_100 | 1 | 52,046 | 0.027 | 2/8 | sqlite | 67,783 | 0.768 | 0.77 |
| range_pk_100 | 32 | 414,977 | 0.144 | 2/8 | cedardb | 527,806 | 0.786 | 1.68 |
| range_secondary_100 | 32 | 1,139,853 | 0.119 | 2/8 | turso | 1,444,486 | 0.789 | 0.79 |
| aggregates_range | 1 | 4,832 | 0.328 | 3/7 | sqlite | 6,122 | 0.789 | 0.79 |
| filter_is_null | 1 | 9,348 | 0.156 | 3/8 | postgres | 11,785 | 0.793 | 0.79 |
| update_range | 1 | 261 | 8.064 | 3/6 | turso | 329 | 0.794 | 0.79 |
| update_indexed | 1 | 264 | 7.808 | 3/6 | sqlite | 323 | 0.818 | 0.82 |
| point_pk | 1 | 343,467 | 0.004 | 2/8 | sqlite | 419,712 | 0.818 | 0.82 |
| blob_read | 32 | 1,285,736 | 0.376 | 2/7 | cedardb | 1,571,122 | 0.818 | 1.34 |
| json_filter | 32 | 1,972 | 41.472 | 3/7 | postgres | 2,391 | 0.825 | 0.82 |
| filter_multi_column | 32 | 5,658 | 83.968 | 2/8 | cedardb | 6,755 | 0.838 | 2.32 |
| update_text | 1 | 290 | 10.240 | 3/6 | sqlite | 323 | 0.900 | 0.90 |
| string_functions | 1 | 26,594 | 0.059 | 3/7 | sqlite | 29,441 | 0.903 | 0.90 |
| delete_insert | 32 | 2,591 | 54.784 | 2/6 | postgres | 2,754 | 0.941 | 0.94 |
| point_text | 1 | 180,119 | 0.012 | 2/8 | sqlite | 190,184 | 0.947 | 0.95 |
| order_by_index | 1 | 134,381 | 0.010 | 2/8 | turso | 139,120 | 0.966 | 0.97 |
| range_secondary_100 | 1 | 154,890 | 0.010 | 1/8 | turso | 152,098 | 1.018 | 1.02 |
| point_secondary | 1 | 184,935 | 0.009 | 1/8 | sqlite | 180,675 | 1.024 | 1.02 |
| like_substring | 32 | 2,601 | 36.352 | 1/8 | postgres | 2,510 | 1.036 | 1.04 |
| point_pk | 32 | 1,315,252 | 0.468 | 1/8 | cedardb | 1,197,765 | 1.098 | 1.38 |
| like_prefix | 1 | 167,318 | 0.010 | 1/8 | sqlite | 141,185 | 1.185 | 1.19 |
| aggregates_range | 32 | 38,864 | 4.032 | 1/7 | turso | 32,265 | 1.205 | 1.20 |
| case_aggregate | 32 | 38,644 | 3.808 | 1/7 | turso | 31,016 | 1.246 | 1.25 |
| join_three | 1 | 1,579 | 0.928 | 1/7 | sqlite | 1,018 | 1.550 | 1.55 |
| update_indexed | 32 | 4,198 | 22.016 | 1/6 | cedardb | 2,541 | 1.652 | 1.73 |
| update_text | 32 | 3,674 | 25.344 | 1/6 | postgres | 2,042 | 1.799 | 1.80 |
| update_plain | 32 | 4,886 | 19.200 | 1/6 | cedardb | 2,527 | 1.934 | 2.13 |
| upsert | 32 | 4,263 | 17.408 | 1/6 | cedardb | 2,191 | 1.946 | 2.08 |
| join_pk | 1 | 73,452 | 0.021 | 1/7 | sqlite | 37,259 | 1.971 | 1.97 |
| update_json | 32 | 4,992 | 14.592 | 1/6 | postgres | 2,493 | 2.002 | 2.00 |
| txn_read_write | 32 | 2,791 | 30.208 | 1/6 | cedardb | 1,226 | 2.276 | 2.75 |
| join_three | 32 | 17,903 | 5.632 | 1/7 | cedardb | 6,474 | 2.765 | 3.75 |
| update_range | 32 | 4,167 | 23.296 | 1/6 | mysql | 1,381 | 3.018 | 3.02 |
| like_prefix | 32 | 972,378 | 0.336 | 1/8 | orioledb | 204,924 | 4.745 | 4.75 |
| in_list_10 | 32 | 443,694 | 0.336 | 1/8 | turso | 82,661 | 5.368 | 5.37 |

## K0: kiln's conversation workload (1,000 sessions, 32 durable writers, 4 readers, 30 s), best of each metric

| target | runs | writes /s | write p99 ms | write p99.9 ms | history read p99 ms | search p50 ms | memory MB (process + server held) |
|---|---:|---:|---:|---:|---:|---:|---:|
| amphora | 3 | 7,601 | 9.34 | 22.27 | 0.109 | 73.73 | 83 |
| postgres | 3 | 5,272 | 10.88 | 65.54 | 0.218 | 10.88 | 253 |
| orioledb | 3 | 3,797 | 29.70 | 176.13 | 0.444 | 21.25 | 756 |
| mysql | 3 | 1,399 | 108.54 | 393.22 | 5.632 | 241.66 | 791 |
| cedardb | 3 | 1,277 | 83.97 | 202.75 | 0.512 | 2.88 | 938 |
| sqlite | 3 | 295 | 2916.35 | 13238.27 | 0.344 | 6.02 | 52 |
| turso | 3 | 289 | 753.66 | 1146.88 | 0.456 | 21.76 | 144 |
| clickhouse | 3 | 162 | 260.10 | 339.97 | 12.288 | 5.44 | 1435 |

