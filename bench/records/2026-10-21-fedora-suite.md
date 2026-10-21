- amphora: load 6.43–4.62, items 200000, load 0.5 s, memory 248.4 MB, commit f597f5a
- turso: load 4.62–3.8, items 200000, load 0.8 s, memory 1164.1 MB, commit f597f5a
- sqlite: load 3.8–5.22, items 200000, load 1.1 s, memory 212.5 MB, commit f597f5a
- postgres: load 5.22–1.72, items 200000, load 3.1 s, memory 261.2 MB, commit f597f5a
- orioledb: load 1.58–1.35, items 200000, load 3.8 s, memory 876.4 MB, commit f597f5a
- cedardb: load 1.32–5.74, items 200000, load 10.8 s, memory 1891.3 MB, commit f597f5a
- mysql: load 5.44–13.3, items 200000, load 3.5 s, memory 685.6 MB, commit f597f5a
- clickhouse: load 12.24–2.56, items 200000, load 124.2 s, memory 2942.3 MB, commit f597f5a

### 1 client

| statement | amphora | cedardb | clickhouse | mysql | orioledb | postgres | sqlite | turso | Amphora / best other |
|---|---|---|---|---|---|---|---|---|---|
| point_pk | 419,328/s p99 0.004 | 63,213/s p99 0.05 | 344/s p99 5.12 | 30,598/s p99 0.132 | 44,925/s p99 0.058 | 50,972/s p99 0.04 | 476,092/s p99 0.003 | 521,280/s p99 0.002 | 0.80x (turso) |
| point_pk_one_column | 430,598/s p99 0.004 | 87,720/s p99 0.026 | 460/s p99 3.04 | 53,828/s p99 0.075 | 53,531/s p99 0.038 | 56,818/s p99 0.044 | 549,726/s p99 0.002 | 562,565/s p99 0.002 | 0.77x (turso) |
| point_secondary | 271,473/s p99 0.006 | 78,060/s p99 0.029 | 388/s p99 5.184 | 43,882/s p99 0.083 | 37,640/s p99 0.067 | 56,864/s p99 0.04 | 199,900/s p99 0.008 | 384,972/s p99 0.004 | 0.71x (turso) |
| point_text | 241,611/s p99 0.008 | 78,885/s p99 0.035 | 169/s p99 7.36 | 36,862/s p99 0.113 | 35,770/s p99 0.064 | 50,068/s p99 0.041 | 201,762/s p99 0.008 | 353,380/s p99 0.006 | 0.68x (turso) |
| in_list_10 | 31,120/s p99 0.051 | 2,942/s p99 0.368 | 251/s p99 5.248 | 20,902/s p99 0.158 | 28,726/s p99 0.07 | 36,414/s p99 0.059 | 94,608/s p99 0.02 | 33,672/s p99 0.044 | 0.33x (sqlite) |
| range_pk_100 | 44,362/s p99 0.035 | 33,190/s p99 0.052 | 378/s p99 4.864 | 22,392/s p99 0.118 | 8,464/s p99 0.238 | 17,726/s p99 0.111 | 59,242/s p99 0.028 | 51,630/s p99 0.031 | 0.75x (sqlite) |
| range_secondary_100 | 149,232/s p99 0.009 | 1,396/s p99 1.04 | 153/s p99 8.192 | 34,467/s p99 0.04 | 10,456/s p99 0.18 | 10,992/s p99 0.176 | 101,670/s p99 0.012 | 188,206/s p99 0.008 | 0.79x (turso) |
| filter_multi_column | 1,220/s p99 2.624 | 4,430/s p99 0.28 | 168/s p99 10.624 | 644/s p99 8.576 | 1,032/s p99 2.496 | 1,822/s p99 1.216 | 2,173/s p99 1.504 | 1,596/s p99 2.112 | 0.28x (cedardb) |
| filter_no_index | 28/s p99 37.376 | 5,492/s p99 0.238 | 296/s p99 4.544 | 20/s p99 77.824 | 42/s p99 27.136 | 86/s p99 15.104 | 51/s p99 25.6 | 34/s p99 30.72 | 0.01x (cedardb) |
| filter_is_null | 4,686/s p99 0.26 | 4,624/s p99 0.264 | 154/s p99 8.32 | 4,234/s p99 0.44 | 2,264/s p99 0.68 | 2,714/s p99 0.508 | 9,414/s p99 0.136 | 9,808/s p99 0.14 | 0.48x (turso) |
| like_prefix | 344/s p99 5.888 | 4,212/s p99 0.328 | 245/s p99 6.016 | 36,178/s p99 0.051 | 12,152/s p99 0.142 | 11,972/s p99 0.154 | 496/s p99 4.16 | 100/s p99 60.928 | 0.01x (mysql) |
| like_substring | 64/s p99 16.64 | 863/s p99 1.264 | 73/s p99 28.416 | 32/s p99 32.512 | 108/s p99 9.984 | 206/s p99 6.976 | 417/s p99 2.528 | 32/s p99 32.768 | 0.07x (cedardb) |
| order_by_index | 63,128/s p99 0.021 | 1,462/s p99 0.832 | 280/s p99 7.168 | 5,732/s p99 0.256 | 22,298/s p99 0.08 | 34,854/s p99 0.053 | 105,828/s p99 0.013 | 72,092/s p99 0.016 | 0.60x (sqlite) |
| order_by_sort | 380/s p99 3.008 | 1,458/s p99 0.896 | 144/s p99 9.216 | 122/s p99 14.08 | 404/s p99 2.72 | 923/s p99 1.344 | 676/s p99 1.632 | 388/s p99 6.272 | 0.26x (cedardb) |
| offset_deep | 69/s p99 31.232 | 702/s p99 2.624 | 134/s p99 11.008 | 86/s p99 56.32 | 47/s p99 43.52 | 114/s p99 19.968 | 134/s p99 15.616 | 24/s p99 47.104 | 0.10x (cedardb) |
| count_range | 160/s p99 7.552 | 5,498/s p99 0.238 | 254/s p99 6.4 | 102/s p99 11.264 | 101/s p99 10.752 | 394/s p99 3.008 | 4,553/s p99 0.256 | 190/s p99 6.656 | 0.03x (cedardb) |
| aggregates_range | 4,242/s p99 0.28 | 5,487/s p99 0.234 | 288/s p99 6.528 | 4,198/s p99 0.96 | 3,445/s p99 0.412 | 7,590/s p99 0.218 | 6,982/s p99 0.188 | 3,164/s p99 0.488 | 0.56x (postgres) |
| group_by_range | 325/s p99 3.584 | 876/s p99 1.744 | 205/s p99 6.976 | 451/s p99 4.544 | 299/s p99 4.0 | 624/s p99 2.464 | 470/s p99 2.336 | 356/s p99 3.168 | 0.37x (cedardb) |
| group_by_having | 356/s p99 3.104 | 462/s p99 2.816 | 220/s p99 7.936 | 146/s p99 12.288 | 364/s p99 2.944 | 803/s p99 1.552 | 586/s p99 1.856 | 418/s p99 2.784 | 0.44x (postgres) |
| distinct_range | 440/s p99 2.4 | 957/s p99 1.776 | 258/s p99 5.504 | 512/s p99 4.096 | 426/s p99 3.712 | 708/s p99 2.08 | 532/s p99 2.0 | 514/s p99 2.144 | 0.46x (cedardb) |
| count_distinct | 358/s p99 2.976 | 396/s p99 3.168 | 204/s p99 6.912 | 143/s p99 13.184 | 391/s p99 2.816 | 902/s p99 1.28 | 569/s p99 1.904 | 428/s p99 2.72 | 0.40x (postgres) |
| join_pk | 100,303/s p99 0.019 | 48,648/s p99 0.044 | 228/s p99 6.464 | 26,161/s p99 0.099 | 1,304/s p99 0.896 | 10,947/s p99 0.4 | 74,954/s p99 0.021 | 78/s p99 14.976 | 1.34x (sqlite) |
| join_group | 65/s p99 18.432 | 576/s p99 2.272 | 228/s p99 5.76 | 18/s p99 99.328 | 41/s p99 31.744 | 76/s p99 16.0 | 110/s p99 10.88 | 74/s p99 15.744 | 0.11x (cedardb) |
| join_three | 1,698/s p99 0.648 | 887/s p99 1.408 | 200/s p99 8.448 | 803/s p99 1.808 | 920/s p99 1.184 | 1,725/s p99 0.696 | 2,392/s p99 0.48 | 70/s p99 23.296 | 0.71x (sqlite) |
| left_join_anti | 458/s p99 3.04 | 4,930/s p99 0.256 | 808/s p99 1.84 | 248/s p99 8.576 | 400/s p99 2.848 | 882/s p99 1.408 | 782/s p99 1.792 | 574/s p99 2.464 | 0.09x (cedardb) |
| subquery_in | 195/s p99 6.912 | 198/s p99 7.744 | 144/s p99 12.8 | 642/s p99 2.592 | 699/s p99 1.888 | 1,544/s p99 0.904 | 252/s p99 5.376 | 210/s p99 6.656 | 0.13x (postgres) |
| exists_subquery | 500/s p99 15.232 | 5,069/s p99 0.28 | 137/s p99 12.16 | 90/s p99 76.8 | 732/s p99 9.088 | 1,800/s p99 3.936 | 840/s p99 8.96 | 644/s p99 12.416 | 0.10x (cedardb) |
| cte | 212/s p99 5.248 | 731/s p99 1.856 | 110/s p99 12.032 | 136/s p99 12.672 | 288/s p99 3.808 | 592/s p99 2.048 | 526/s p99 2.304 | 237/s p99 4.544 | 0.29x (cedardb) |
| union_all | 251,292/s p99 0.006 | 74,676/s p99 0.031 | 142/s p99 8.832 | 42,936/s p99 0.048 | 27,332/s p99 0.062 | 58,043/s p99 0.038 | 172,050/s p99 0.008 | 473,391/s p99 0.002 | 0.53x (turso) |
| case_aggregate | 4,093/s p99 0.284 | 5,605/s p99 0.238 | 224/s p99 6.464 | 3,913/s p99 0.96 | 3,582/s p99 0.336 | 7,400/s p99 0.218 | 7,743/s p99 0.164 | 4,850/s p99 0.25 | 0.53x (sqlite) |
| window_row_number | 977/s p99 1.072 | 2,206/s p99 0.712 | 214/s p99 6.336 | 2,424/s p99 0.936 | 1,239/s p99 1.056 | 2,228/s p99 0.568 | 1,898/s p99 0.592 | 1,036/s p99 1.008 | 0.40x (mysql) |
| string_functions | 24,914/s p99 0.062 | 19,077/s p99 0.107 | 332/s p99 5.76 | 14,275/s p99 0.208 | 5,581/s p99 0.228 | 9,052/s p99 0.184 | 27,942/s p99 0.059 | 26,944/s p99 0.053 | 0.89x (sqlite) |
| date_bucket | 277/s p99 3.936 | 1,100/s p99 1.344 | 232/s p99 6.976 | 301/s p99 5.376 | 107/s p99 9.6 | 228/s p99 5.12 | 384/s p99 2.752 | 314/s p99 3.456 | 0.25x (cedardb) |
| json_field | 353,040/s p99 0.005 | 76,138/s p99 0.028 | 358/s p99 3.552 | 56,233/s p99 0.074 | 41,945/s p99 0.089 | 49,964/s p99 0.058 | 494,942/s p99 0.004 | 444,058/s p99 0.003 | 0.71x (sqlite) |
| json_filter | 320/s p99 3.808 | 5,155/s p99 0.254 | 168/s p99 12.544 | 180/s p99 11.52 | 372/s p99 3.136 | 778/s p99 1.632 | 594/s p99 1.968 | 372/s p99 3.264 | 0.06x (cedardb) |
| blob_read | 424,254/s p99 0.003 | 64,849/s p99 0.033 | 414/s p99 3.488 | 59,252/s p99 0.072 | 52,988/s p99 0.038 | 67,494/s p99 0.029 | 537,514/s p99 0.002 | 560,920/s p99 0.002 | 0.76x (turso) |
| text_read | 417,636/s p99 0.005 | 67,558/s p99 0.031 | 242/s p99 8.576 | 63,691/s p99 0.069 | 53,018/s p99 0.027 | 63,804/s p99 0.034 | 553,913/s p99 0.002 | 549,548/s p99 0.002 | 0.75x (sqlite) |
| table_group_by | 4/s p99 262.144 | 862/s p99 1.776 | 170/s p99 9.6 | 2/s p99 688.128 | 34/s p99 32.512 | 62/s p99 18.944 | 6/s p99 153.6 | 4/s p99 219.136 | 0.00x (cedardb) |
| table_count | 2,128/s p99 0.544 | 5,572/s p99 0.24 | 1,430/s p99 1.04 | 125/s p99 8.96 | 127/s p99 9.856 | 194/s p99 5.504 | 6,238/s p99 0.188 | 43,826/s p99 0.027 | 0.05x (turso) |
| insert_one | 298/s p99 5.76 | 90/s p99 13.312 | 16/s p99 64.0 | 132/s p99 19.456 | 353/s p99 3.392 | 334/s p99 3.936 | 297/s p99 8.128 | 305/s p99 6.08 | 0.85x (orioledb) |
| insert_batch_100 | 168/s p99 9.472 | 65/s p99 22.272 | 9/s p99 212.992 | 68/s p99 57.856 | 178/s p99 8.128 | 180/s p99 7.872 | 106/s p99 25.856 | 94/s p99 28.672 | 0.94x (postgres) |
| update_plain | 299/s p99 5.76 | 88/s p99 27.648 | unsupported | 146/s p99 11.52 | 332/s p99 3.84 | 322/s p99 5.12 | 326/s p99 3.968 | 321/s p99 3.936 | 0.90x (orioledb) |
| update_indexed | 296/s p99 5.952 | 91/s p99 12.544 | unsupported | 122/s p99 19.2 | 348/s p99 3.456 | 310/s p99 10.496 | 328/s p99 3.776 | 304/s p99 4.288 | 0.85x (orioledb) |
| update_text | 306/s p99 5.696 | 91/s p99 12.672 | unsupported | 146/s p99 14.08 | 340/s p99 3.872 | 348/s p99 3.776 | 311/s p99 4.352 | 322/s p99 6.016 | 0.88x (postgres) |
| update_json | 294/s p99 6.016 | 91/s p99 12.416 | unsupported | 144/s p99 15.488 | 346/s p99 3.584 | 350/s p99 3.744 | 351/s p99 3.552 | 326/s p99 3.904 | 0.84x (sqlite) |
| update_range | 294/s p99 5.824 | 91/s p99 12.544 | unsupported | 141/s p99 15.488 | 298/s p99 3.968 | 276/s p99 4.608 | 342/s p99 3.84 | 308/s p99 4.16 | 0.86x (sqlite) |
| upsert | 293/s p99 5.888 | 90/s p99 12.544 | unsupported | 148/s p99 13.056 | 337/s p99 3.84 | 339/s p99 3.872 | 346/s p99 3.648 | 319/s p99 4.032 | 0.85x (sqlite) |
| delete_insert | 286/s p99 5.888 | 90/s p99 13.696 | unsupported | 129/s p99 14.336 | 304/s p99 4.736 | 295/s p99 4.736 | 308/s p99 5.12 | 312/s p99 5.504 | 0.92x (turso) |
| delete_range_insert | 250/s p99 6.464 | 90/s p99 12.288 | unsupported | 108/s p99 22.272 | 236/s p99 5.376 | 259/s p99 5.056 | 208/s p99 24.832 | 200/s p99 31.488 | 0.96x (postgres) |
| txn_read_write | 266/s p99 6.208 | 89/s p99 17.664 | unsupported | 117/s p99 22.016 | 279/s p99 4.992 | 294/s p99 4.544 | 314/s p99 5.12 | 310/s p99 6.208 | 0.85x (sqlite) |

### 32 clients

| statement | amphora | cedardb | clickhouse | mysql | orioledb | postgres | sqlite | turso | Amphora / best other |
|---|---|---|---|---|---|---|---|---|---|
| point_pk | 1,047,358/s p99 0.09 | 1,157,704/s p99 0.044 | 3,194/s p99 18.688 | 487,068/s p99 0.16 | 647,708/s p99 0.08 | 884,786/s p99 0.057 | 329,168/s p99 0.206 | 368,596/s p99 0.296 | 0.90x (cedardb) |
| point_pk_one_column | 1,050,910/s p99 0.091 | 794,907/s p99 0.102 | 12,598/s p99 4.8 | 629,569/s p99 0.128 | 722,510/s p99 0.077 | 969,237/s p99 0.053 | 335,481/s p99 0.196 | 370,269/s p99 0.3 | 1.08x (postgres) |
| point_secondary | 1,009,380/s p99 0.092 | 1,207,174/s p99 0.041 | 6,556/s p99 9.216 | 486,462/s p99 0.168 | 509,217/s p99 0.107 | 970,514/s p99 0.054 | 68,385/s p99 0.928 | 320,562/s p99 0.352 | 0.84x (cedardb) |
| point_text | 1,015,092/s p99 0.096 | 1,136,712/s p99 0.079 | 4,611/s p99 14.336 | 468,774/s p99 0.17 | 492,332/s p99 0.178 | 944,951/s p99 0.055 | 77,409/s p99 0.848 | 297,595/s p99 0.368 | 0.89x (cedardb) |
| in_list_10 | 147,240/s p99 0.304 | 4,034/s p99 12.928 | 5,332/s p99 11.008 | 254,554/s p99 0.3 | 392,463/s p99 0.126 | 415,068/s p99 0.18 | 49,418/s p99 0.912 | 150,056/s p99 1.6 | 0.35x (postgres) |
| range_pk_100 | 139,220/s p99 0.648 | 419,096/s p99 0.116 | 8,395/s p99 7.104 | 266,878/s p99 0.276 | 130,730/s p99 0.32 | 253,887/s p99 0.288 | 52,894/s p99 0.928 | 43,224/s p99 2.72 | 0.33x (cedardb) |
| range_secondary_100 | 1,049,500/s p99 0.079 | 38,456/s p99 3.616 | 3,240/s p99 16.896 | 403,816/s p99 0.112 | 146,774/s p99 0.464 | 166,844/s p99 0.408 | 68,812/s p99 0.728 | 1,558,548/s p99 0.051 | 0.67x (turso) |
| filter_multi_column | 3,668/s p99 48.64 | 32,166/s p99 3.392 | 2,646/s p99 19.968 | 3,928/s p99 28.928 | 15,238/s p99 5.504 | 32,731/s p99 2.56 | 818/s p99 129.024 | 1,324/s p99 135.168 | 0.11x (postgres) |
| filter_no_index | 72/s p99 835.584 | 39,566/s p99 3.168 | 3,393/s p99 15.872 | 512/s p99 74.752 | 392/s p99 153.6 | 916/s p99 56.32 | 33/s p99 1130.496 | 30/s p99 1081.344 | 0.00x (cedardb) |
| filter_is_null | 10,508/s p99 6.336 | 37,231/s p99 3.328 | 3,187/s p99 16.64 | 10,830/s p99 5.184 | 33,695/s p99 1.776 | 49,180/s p99 0.728 | 3,037/s p99 13.824 | 8,795/s p99 12.928 | 0.21x (postgres) |
| like_prefix | 4,262/s p99 18.432 | 7,196/s p99 8.32 | 3,578/s p99 15.104 | 444,844/s p99 0.17 | 162,062/s p99 0.28 | 80,856/s p99 4.096 | 2,228/s p99 30.976 | 3,031/s p99 36.864 | 0.01x (mysql) |
| like_substring | 836/s p99 56.832 | 1,050/s p99 43.52 | 566/s p99 114.688 | 608/s p99 92.16 | 1,534/s p99 23.808 | 2,920/s p99 22.784 | 281/s p99 137.216 | 410/s p99 139.264 | 0.29x (postgres) |
| order_by_index | 172,590/s p99 0.552 | 28,934/s p99 3.744 | 3,672/s p99 16.384 | 14,873/s p99 3.136 | 293,605/s p99 0.144 | 599,314/s p99 0.073 | 47,146/s p99 0.976 | 1,094,852/s p99 0.034 | 0.16x (turso) |
| order_by_sort | 1,084/s p99 56.832 | 34,303/s p99 3.264 | 2,586/s p99 20.992 | 1,393/s p99 27.136 | 6,042/s p99 5.888 | 15,176/s p99 3.616 | 352/s p99 98.304 | 411/s p99 163.84 | 0.03x (cedardb) |
| offset_deep | 134/s p99 778.24 | 1,910/s p99 24.832 | 2,818/s p99 20.224 | 1,312/s p99 75.776 | 644/s p99 107.52 | 1,392/s p99 61.44 | 44/s p99 1064.96 | 254/s p99 157.696 | 0.05x (clickhouse) |
| count_range | 2,615/s p99 20.224 | 48,080/s p99 3.584 | 7,365/s p99 9.088 | 1,473/s p99 24.064 | 1,459/s p99 25.344 | 5,622/s p99 6.848 | 3,527/s p99 11.008 | 2,942/s p99 26.368 | 0.05x (cedardb) |
| aggregates_range | 26,875/s p99 4.608 | 47,868/s p99 3.424 | 6,316/s p99 9.344 | 66,332/s p99 1.568 | 49,253/s p99 1.024 | 97,636/s p99 0.632 | 6,266/s p99 6.656 | 5,287/s p99 23.04 | 0.28x (postgres) |
| group_by_range | 3,764/s p99 18.944 | 30,575/s p99 4.0 | 6,156/s p99 9.984 | 6,946/s p99 9.856 | 5,378/s p99 8.96 | 9,166/s p99 6.464 | 630/s p99 61.44 | 483/s p99 126.976 | 0.12x (cedardb) |
| group_by_having | 1,224/s p99 56.832 | 20,879/s p99 4.16 | 3,283/s p99 17.408 | 1,392/s p99 25.344 | 5,339/s p99 6.592 | 12,390/s p99 4.416 | 352/s p99 100.352 | 372/s p99 208.896 | 0.06x (cedardb) |
| distinct_range | 1,868/s p99 33.792 | 29,822/s p99 3.904 | 8,000/s p99 7.744 | 8,180/s p99 8.576 | 6,221/s p99 7.744 | 10,428/s p99 6.08 | 634/s p99 60.928 | 543/s p99 129.024 | 0.06x (cedardb) |
| count_distinct | 1,158/s p99 54.272 | 21,882/s p99 3.872 | 3,624/s p99 16.128 | 1,408/s p99 27.136 | 5,938/s p99 5.952 | 14,631/s p99 3.84 | 320/s p99 107.52 | 412/s p99 202.752 | 0.05x (cedardb) |
| join_pk | 851,821/s p99 0.079 | 373,286/s p99 0.206 | 6,153/s p99 9.472 | 184,798/s p99 0.4 | 18,862/s p99 2.016 | 146,300/s p99 0.84 | 30,222/s p99 1.808 | 1,240/s p99 53.248 | 2.28x (cedardb) |
| join_group | 292/s p99 258.048 | 14,260/s p99 5.44 | 2,155/s p99 25.088 | 272/s p99 139.264 | 374/s p99 155.648 | 784/s p99 61.952 | 67/s p99 548.864 | 76/s p99 917.504 | 0.02x (cedardb) |
| join_three | 22,602/s p99 4.416 | 32,900/s p99 3.84 | 4,420/s p99 12.928 | 3,454/s p99 17.408 | 13,190/s p99 2.816 | 23,920/s p99 2.656 | 1,336/s p99 26.624 | 1,064/s p99 35.328 | 0.69x (cedardb) |
| left_join_anti | 1,257/s p99 58.368 | 39,180/s p99 3.328 | 10,870/s p99 5.632 | 1,843/s p99 28.16 | 6,018/s p99 5.824 | 14,964/s p99 3.616 | 328/s p99 133.12 | 446/s p99 172.032 | 0.03x (cedardb) |
| subquery_in | 2,937/s p99 18.432 | 578/s p99 68.608 | 1,938/s p99 27.648 | 2,605/s p99 17.92 | 10,395/s p99 4.096 | 25,975/s p99 1.568 | 135/s p99 311.296 | 1,130/s p99 73.728 | 0.11x (postgres) |
| exists_subquery | 1,361/s p99 251.904 | 38,676/s p99 3.488 | 2,546/s p99 22.016 | 1,341/s p99 133.12 | 10,687/s p99 18.944 | 25,777/s p99 9.984 | 284/s p99 688.128 | 390/s p99 770.048 | 0.04x (cedardb) |
| cte | 2,550/s p99 27.392 | 20,330/s p99 4.096 | 2,140/s p99 24.32 | 1,328/s p99 25.856 | 4,162/s p99 8.96 | 9,386/s p99 6.144 | 291/s p99 115.712 | 374/s p99 196.608 | 0.13x (cedardb) |
| union_all | 1,018,610/s p99 0.096 | 977,850/s p99 0.083 | 3,808/s p99 17.408 | 500,136/s p99 0.112 | 308,280/s p99 0.172 | 748,498/s p99 0.082 | 58,555/s p99 0.968 | 1,437,148/s p99 0.111 | 0.71x (turso) |
| case_aggregate | 16,766/s p99 5.184 | 48,344/s p99 3.392 | 6,398/s p99 8.96 | 68,010/s p99 1.392 | 51,190/s p99 0.68 | 97,632/s p99 0.704 | 6,217/s p99 6.72 | 5,564/s p99 12.672 | 0.17x (postgres) |
| window_row_number | 11,167/s p99 6.272 | 26,292/s p99 3.648 | 6,836/s p99 8.704 | 34,020/s p99 2.464 | 18,730/s p99 2.048 | 25,954/s p99 2.24 | 1,296/s p99 27.136 | 12,246/s p99 3.04 | 0.33x (mysql) |
| string_functions | 289,004/s p99 0.15 | 289,996/s p99 0.664 | 7,894/s p99 7.68 | 173,874/s p99 0.412 | 83,126/s p99 0.448 | 131,949/s p99 0.484 | 10,994/s p99 7.872 | 48,032/s p99 1.616 | 1.00x (cedardb) |
| date_bucket | 3,384/s p99 19.456 | 23,938/s p99 4.096 | 6,000/s p99 9.984 | 315/s p99 111.616 | 1,586/s p99 22.272 | 3,050/s p99 18.688 | 198/s p99 212.992 | 542/s p99 121.856 | 0.14x (cedardb) |
| json_field | 1,033,670/s p99 0.097 | 745,754/s p99 0.107 | 11,154/s p99 5.248 | 595,930/s p99 0.132 | 633,878/s p99 0.093 | 899,890/s p99 0.101 | 325,200/s p99 0.202 | 372,596/s p99 0.292 | 1.15x (postgres) |
| json_filter | 2,916/s p99 22.784 | 35,050/s p99 3.552 | 2,027/s p99 27.136 | 1,557/s p99 28.16 | 5,673/s p99 11.52 | 13,260/s p99 4.288 | 293/s p99 139.264 | 419/s p99 161.792 | 0.08x (cedardb) |
| blob_read | 1,040,506/s p99 0.1 | 740,538/s p99 0.108 | 9,764/s p99 6.272 | 623,886/s p99 0.13 | 678,602/s p99 0.086 | 931,266/s p99 0.067 | 334,154/s p99 0.198 | 372,874/s p99 0.296 | 1.12x (postgres) |
| text_read | 1,036,462/s p99 0.107 | 1,103,686/s p99 0.05 | 5,610/s p99 12.416 | 628,144/s p99 0.128 | 644,280/s p99 0.091 | 933,380/s p99 0.054 | 333,226/s p99 0.204 | 372,443/s p99 0.296 | 0.94x (cedardb) |
| table_group_by | 7/s p99 1851.392 | 15,562/s p99 4.864 | 3,042/s p99 17.664 | 16/s p99 2490.368 | 268/s p99 227.328 | 516/s p99 101.376 | 0/s p99 – | 0/s p99 – | 0.00x (cedardb) |
| table_count | 3,162/s p99 23.552 | 50,286/s p99 3.072 | 17,970/s p99 3.04 | 1,442/s p99 40.448 | 1,198/s p99 53.76 | 2,858/s p99 13.44 | 4,008/s p99 14.72 | 685,844/s p99 0.052 | 0.00x (turso) |
| insert_one | 5,766/s p99 8.832 | 2,087/s p99 22.016 | 148/s p99 212.992 | 2,296/s p99 29.44 | 5,483/s p99 6.848 | 5,200/s p99 7.36 | 322/s p99 22.528 | 291/s p99 499.712 | 1.05x (orioledb) |
| insert_batch_100 | 394/s p99 157.696 | 480/s p99 397.312 | 148/s p99 223.232 | 470/s p99 139.264 | 2,814/s p99 26.624 | 1,854/s p99 52.224 | 127/s p99 1441.792 | 115/s p99 1064.96 | 0.14x (orioledb) |
| update_plain | 5,296/s p99 9.088 | 2,097/s p99 22.784 | unsupported | 2,408/s p99 33.28 | 5,376/s p99 7.104 | 4,862/s p99 10.368 | 334/s p99 3.968 | 319/s p99 638.976 | 0.99x (orioledb) |
| update_indexed | 7,856/s p99 6.528 | 2,098/s p99 22.784 | unsupported | 2,225/s p99 35.328 | 5,196/s p99 14.976 | 5,397/s p99 7.552 | 315/s p99 15.36 | 304/s p99 548.864 | 1.46x (postgres) |
| update_text | 7,172/s p99 9.216 | 2,083/s p99 22.784 | unsupported | 2,199/s p99 36.352 | 5,384/s p99 7.104 | 5,588/s p99 7.808 | 355/s p99 3.616 | 325/s p99 581.632 | 1.28x (postgres) |
| update_json | 5,980/s p99 8.576 | 1,968/s p99 22.528 | unsupported | 2,418/s p99 40.448 | 5,422/s p99 7.04 | 5,574/s p99 7.36 | 358/s p99 3.68 | 316/s p99 557.056 | 1.07x (postgres) |
| update_range | 5,683/s p99 9.984 | 1,465/s p99 31.488 | unsupported | 1,788/s p99 47.616 | 5,094/s p99 9.216 | 4,537/s p99 12.672 | 346/s p99 3.744 | 306/s p99 638.976 | 1.12x (orioledb) |
| upsert | 8,032/s p99 6.464 | 2,106/s p99 23.296 | unsupported | 2,539/s p99 29.184 | 5,492/s p99 7.296 | 5,198/s p99 8.064 | 336/s p99 5.696 | 314/s p99 565.248 | 1.46x (orioledb) |
| delete_insert | 5,518/s p99 9.6 | 1,678/s p99 24.832 | unsupported | 2,220/s p99 35.84 | 5,476/s p99 6.72 | 5,172/s p99 7.744 | 316/s p99 12.288 | 302/s p99 598.016 | 1.01x (orioledb) |
| delete_range_insert | 2,708/s p99 36.352 | 1,337/s p99 33.792 | unsupported | 1,631/s p99 41.984 | 4,688/s p99 10.496 | 4,034/s p99 15.104 | 152/s p99 933.888 | 216/s p99 704.512 | 0.58x (orioledb) |
| txn_read_write | 5,008/s p99 10.88 | 1,520/s p99 25.856 | unsupported | 1,788/s p99 38.4 | 5,374/s p99 6.72 | 5,468/s p99 8.576 | 307/s p99 27.136 | 300/s p99 503.808 | 0.92x (postgres) |

### Where Amphora loses, worst first (its throughput over the best other's)

- table_group_by (Read, 32 clients): 0.00x of cedardb — GROUP BY over the whole table
- filter_no_index (Read, 32 clients): 0.00x of cedardb — COUNT(*) of a filter no index serves: a whole scan
- table_count (Read, 32 clients): 0.00x of turso — COUNT(*) of the whole table
- table_group_by (Read, 1 clients): 0.00x of cedardb — GROUP BY over the whole table
- filter_no_index (Read, 1 clients): 0.01x of cedardb — COUNT(*) of a filter no index serves: a whole scan
- like_prefix (Read, 1 clients): 0.01x of mysql — LIKE 'prefix%' on an indexed text column
- like_prefix (Read, 32 clients): 0.01x of mysql — LIKE 'prefix%' on an indexed text column
- join_group (Read, 32 clients): 0.02x of cedardb — a join filtered on the other table, grouped and ordered
- count_range (Read, 1 clients): 0.03x of cedardb — COUNT(*) over a range of a timestamp: 10,000 rows
- order_by_sort (Read, 32 clients): 0.03x of cedardb — ORDER BY an unindexed column, LIMIT 20: a sort
- left_join_anti (Read, 32 clients): 0.03x of cedardb — LEFT JOIN ... IS NULL: rows with no match
- exists_subquery (Read, 32 clients): 0.04x of cedardb — EXISTS (SELECT ...) correlated
- offset_deep (Read, 32 clients): 0.05x of clickhouse — LIMIT 20 OFFSET up to the table's size
- table_count (Read, 1 clients): 0.05x of turso — COUNT(*) of the whole table
- count_distinct (Read, 32 clients): 0.05x of cedardb — COUNT(DISTINCT) within an indexed equality
- count_range (Read, 32 clients): 0.05x of cedardb — COUNT(*) over a range of a timestamp: 10,000 rows
- group_by_having (Read, 32 clients): 0.06x of cedardb — GROUP BY, HAVING, ORDER BY the count, LIMIT
- json_filter (Read, 1 clients): 0.06x of cedardb — rows filtered on a JSON document's field
- distinct_range (Read, 32 clients): 0.06x of cedardb — DISTINCT over 10,000 rows
- like_substring (Read, 1 clients): 0.07x of cedardb — LIKE '%word%' on long text: a whole scan
- json_filter (Read, 32 clients): 0.08x of cedardb — rows filtered on a JSON document's field
- left_join_anti (Read, 1 clients): 0.09x of cedardb — LEFT JOIN ... IS NULL: rows with no match
- offset_deep (Read, 1 clients): 0.10x of cedardb — LIMIT 20 OFFSET up to the table's size
- exists_subquery (Read, 1 clients): 0.10x of cedardb — EXISTS (SELECT ...) correlated
- filter_multi_column (Read, 32 clients): 0.11x of postgres — an indexed equality, a range and a boolean, LIMIT
- join_group (Read, 1 clients): 0.11x of cedardb — a join filtered on the other table, grouped and ordered
- subquery_in (Read, 32 clients): 0.11x of postgres — IN (SELECT ...) beside an indexed equality
- group_by_range (Read, 32 clients): 0.12x of cedardb — GROUP BY over 10,000 rows
- cte (Read, 32 clients): 0.13x of cedardb — WITH ... a grouped CTE joined and ordered
- subquery_in (Read, 1 clients): 0.13x of postgres — IN (SELECT ...) beside an indexed equality
- insert_batch_100 (Write, 32 clients): 0.14x of orioledb — a hundred rows in one statement
- date_bucket (Read, 32 clients): 0.14x of cedardb — rows counted by month from a timestamp, 10,000 rows
- order_by_index (Read, 32 clients): 0.16x of turso — ORDER BY an index's second column, DESC, LIMIT 20
- case_aggregate (Read, 32 clients): 0.17x of postgres — SUM(CASE ...) over 1,000 rows
- filter_is_null (Read, 32 clients): 0.21x of postgres — an indexed equality and IS NULL, LIMIT
- date_bucket (Read, 1 clients): 0.25x of cedardb — rows counted by month from a timestamp, 10,000 rows
- order_by_sort (Read, 1 clients): 0.26x of cedardb — ORDER BY an unindexed column, LIMIT 20: a sort
- filter_multi_column (Read, 1 clients): 0.28x of cedardb — an indexed equality, a range and a boolean, LIMIT
- aggregates_range (Read, 32 clients): 0.28x of postgres — SUM, AVG, MIN, MAX over 1,000 rows
- like_substring (Read, 32 clients): 0.29x of postgres — LIKE '%word%' on long text: a whole scan
- cte (Read, 1 clients): 0.29x of cedardb — WITH ... a grouped CTE joined and ordered
- window_row_number (Read, 32 clients): 0.33x of mysql — ROW_NUMBER() OVER (PARTITION BY ... ORDER BY ...) over 1,000 rows
- in_list_10 (Read, 1 clients): 0.33x of sqlite — ten rows by primary key in an IN list
- range_pk_100 (Read, 32 clients): 0.33x of cedardb — a hundred rows by a primary-key range
- in_list_10 (Read, 32 clients): 0.35x of postgres — ten rows by primary key in an IN list
- group_by_range (Read, 1 clients): 0.37x of cedardb — GROUP BY over 10,000 rows
- count_distinct (Read, 1 clients): 0.40x of postgres — COUNT(DISTINCT) within an indexed equality
- window_row_number (Read, 1 clients): 0.40x of mysql — ROW_NUMBER() OVER (PARTITION BY ... ORDER BY ...) over 1,000 rows
- group_by_having (Read, 1 clients): 0.44x of postgres — GROUP BY, HAVING, ORDER BY the count, LIMIT
- distinct_range (Read, 1 clients): 0.46x of cedardb — DISTINCT over 10,000 rows
- filter_is_null (Read, 1 clients): 0.48x of turso — an indexed equality and IS NULL, LIMIT
- case_aggregate (Read, 1 clients): 0.53x of sqlite — SUM(CASE ...) over 1,000 rows
- union_all (Read, 1 clients): 0.53x of turso — UNION ALL of two index lookups
- aggregates_range (Read, 1 clients): 0.56x of postgres — SUM, AVG, MIN, MAX over 1,000 rows
- delete_range_insert (Write, 32 clients): 0.58x of orioledb — ten rows deleted by range and put back, one transaction
- order_by_index (Read, 1 clients): 0.60x of sqlite — ORDER BY an index's second column, DESC, LIMIT 20
- range_secondary_100 (Read, 32 clients): 0.67x of turso — about a hundred rows by a secondary-index range
- point_text (Read, 1 clients): 0.68x of turso — rows by an indexed text column
- join_three (Read, 32 clients): 0.69x of cedardb — three tables joined over 1,000 rows, grouped
- point_secondary (Read, 1 clients): 0.71x of turso — rows by a secondary integer index
- union_all (Read, 32 clients): 0.71x of turso — UNION ALL of two index lookups
- join_three (Read, 1 clients): 0.71x of sqlite — three tables joined over 1,000 rows, grouped
- json_field (Read, 1 clients): 0.71x of sqlite — a JSON document's field, by primary key
- range_pk_100 (Read, 1 clients): 0.75x of sqlite — a hundred rows by a primary-key range
- text_read (Read, 1 clients): 0.75x of sqlite — 400 bytes of text by primary key
- blob_read (Read, 1 clients): 0.76x of turso — a 64-byte blob by primary key
- point_pk_one_column (Read, 1 clients): 0.77x of turso — one column of a row by primary key
- range_secondary_100 (Read, 1 clients): 0.79x of turso — about a hundred rows by a secondary-index range
- point_pk (Read, 1 clients): 0.80x of turso — a row by primary key, every column
- point_secondary (Read, 32 clients): 0.84x of cedardb — rows by a secondary integer index
- update_json (Write, 1 clients): 0.84x of sqlite — a JSON document of a row by key
- upsert (Write, 1 clients): 0.85x of sqlite — INSERT ... ON CONFLICT DO UPDATE on an existing key
- insert_one (Write, 1 clients): 0.85x of orioledb — one row, every type
- txn_read_write (Write, 1 clients): 0.85x of sqlite — a transaction that reads two rows, updates them and inserts one
- update_indexed (Write, 1 clients): 0.85x of orioledb — an indexed column of a row by key
- update_range (Write, 1 clients): 0.86x of sqlite — a boolean of ten rows by a key range
- update_text (Write, 1 clients): 0.88x of postgres — 400 bytes of text of a row by key
- string_functions (Read, 1 clients): 0.89x of sqlite — UPPER, LENGTH, SUBSTR over 100 rows
- point_text (Read, 32 clients): 0.89x of cedardb — rows by an indexed text column
- update_plain (Write, 1 clients): 0.90x of orioledb — a real and an integer of a row by key
- point_pk (Read, 32 clients): 0.90x of cedardb — a row by primary key, every column
- txn_read_write (Write, 32 clients): 0.92x of postgres — a transaction that reads two rows, updates them and inserts one
- delete_insert (Write, 1 clients): 0.92x of turso — a row deleted by key and put back, one transaction
- insert_batch_100 (Write, 1 clients): 0.94x of postgres — a hundred rows in one statement
- text_read (Read, 32 clients): 0.94x of cedardb — 400 bytes of text by primary key
- delete_range_insert (Write, 1 clients): 0.96x of postgres — ten rows deleted by range and put back, one transaction
- update_plain (Write, 32 clients): 0.99x of orioledb — a real and an integer of a row by key
- string_functions (Read, 32 clients): 1.00x of cedardb — UPPER, LENGTH, SUBSTR over 100 rows
