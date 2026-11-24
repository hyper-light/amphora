| query | plan A = B | plan B = C | filters A | filters B | filters C | A ms | B ms | C ms | p B vs A | p C vs B | load range |
|---|---|---|---|---|---|---|---|---|---|---|---|
| q01 | yes | yes | none | none | none | 3,523 (2,957 to 4,286) | 3,257 (2,398 to 4,056) | 2,571 (2,338 to 3,193) | 0.485 | 0.093 | 31.3 to 62.1 |
| q02 | yes | yes | none | none | none | 49.4 (36.6 to 70.8) | 53.2 (27.5 to 101) | 36.9 (31.6 to 50.2) | 0.937 | 0.589 | 31.4 to 60.8 |
| q03 | NO | yes | none | customer | customer | 2,962 (2,572 to 17,934) | 1,016 (787 to 11,293) | 1,076 (908 to 1,388) | 0.041 | 0.485 | 32.7 to 62.3 |
| q04 | yes | yes | none | none | none | 421 (319 to 585) | 378 (290 to 670) | 357 (337 to 431) | 0.699 | 1.000 | 32.2 to 60.9 |
| q05 | yes | yes | none | none | none | 1,890 (1,780 to 2,369) | 1,747 (1,490 to 2,389) | 1,652 (1,458 to 2,115) | 0.589 | 0.937 | 32.2 to 60.1 |
| q06 | yes | yes | none | none | none | 891 (826 to 1,003) | 857 (755 to 1,027) | 614 (550 to 699) | 0.485 | 0.002 | 31.5 to 56.7 |
| q07 | yes | yes | none | none | none | 1,428 (1,154 to 1,863) | 1,499 (1,200 to 1,901) | 956 (851 to 1,043) | 0.699 | 0.002 | 31.5 to 56.3 |
| q08 | yes | yes | none | none | none | 407 (357 to 983) | 286 (237 to 460) | 218 (195 to 233) | 0.065 | 0.002 | 31.4 to 54.7 |
| q09 | yes | yes | none | none | none | 3,073 (2,466 to 5,990) | 2,511 (2,152 to 3,286) | 1,956 (1,715 to 2,062) | 0.132 | 0.002 | 30.7 to 54.7 |
| q10 | NO | yes | none | orders | orders | 1,157 (1,069 to 1,892) | 1,467 (1,245 to 1,921) | 1,337 (1,018 to 2,131) | 0.093 | 0.699 | 30.4 to 54.4 |
| q11 | yes | yes | none | none | none | 179 (145 to 221) | 158 (121 to 281) | 170 (104 to 1,838) | 0.699 | 0.818 | 30.4 to 54.4 |
| q12 | yes | yes | none | none | none | 809 (684 to 1,031) | 952 (724 to 1,248) | 969 (800 to 1,367) | 0.310 | 0.485 | 29.4 to 54.4 |
| q13 | yes | yes | none | none | none | 3,112 (2,830 to 4,098) | 4,063 (2,383 to 4,881) | 2,948 (2,655 to 3,436) | 0.485 | 0.394 | 29.1 to 56.5 |
| q14 | yes | yes | none | none | none | 991 (767 to 1,112) | 681 (638 to 832) | 717 (647 to 797) | 0.004 | 0.394 | 29.7 to 56.3 |
| q15 | yes | yes | none | none | none | 1,535 (1,448 to 1,708) | 1,544 (1,348 to 1,681) | 1,670 (1,348 to 1,699) | 1.000 | 0.394 | 28.6 to 56.3 |
| q16 | yes | yes | none | none | none | 181 (175 to 193) | 192 (170 to 258) | 211 (202 to 226) | 0.818 | 0.699 | 28.6 to 54.0 |
| q17 | yes | yes | none | none | none | 98.0 (95.3 to 98.8) | 96.5 (86.4 to 116) | 117 (106 to 130) | 1.000 | 0.015 | 28.6 to 54.0 |
| q18 | yes | yes | none | none | none | 2,266 (2,012 to 2,519) | 2,057 (1,871 to 2,358) | 2,714 (2,320 to 3,643) | 0.093 | 0.004 | 29.7 to 52.1 |
| q19 | yes | yes | none | none | none | 54.4 (44.8 to 67.9) | 56.9 (41.1 to 72.3) | 77.3 (56.4 to 106) | 0.937 | 0.180 | 30.1 to 51.0 |
| q20 | NO | yes | none | nation | nation | 541 (425 to 645) | 505 (334 to 1,300) | 626 (407 to 1,472) | 1.000 | 0.589 | 29.2 to 58.3 |
| q21 | yes | yes | none | none | none | 1,395 (1,200 to 1,841) | 1,411 (842 to 3,814) | 1,246 (1,076 to 2,275) | 1.000 | 0.818 | 29.2 to 61.3 |
| q22 | yes | yes | none | none | none | 144 (120 to 205) | 133 (76.6 to 244) | 135 (92.6 to 199) | 0.699 | 1.000 | 30.7 to 61.3 |

failed runs: 0 of 396; counted-run rows per operator identical B vs C: 22 of 22 queries

The plans (B's; A's too where they differ; C's are B's on every query):

- q01 B: SCAN lineitem / USE HASH TABLE FOR GROUP BY
- q02 B: MATERIALIZE hash build input for supplier / SCAN part / SEARCH partsupp USING INDEX sqlite_autoindex_partsupp_1 (ps_partkey=?) / SEARCH supplier USING INTEGER PRIMARY KEY (rowid=?) / CORRELATED SCALAR SUBQUERY 1 / SEARCH partsupp USING INDEX sqlite_autoindex_partsupp_1 (ps_partkey=?) / SEARCH supplier USING INTEGER PRIMARY KEY (rowid=?) / SEARCH nation USING INTEGER PRIMARY KEY (rowid=?) / SEARCH region USING INTEGER PRIMARY KEY (rowid=?) / SCAN region / HASH JOIN nation / CORRELATED SCALAR SUBQUERY 2 / SEARCH partsupp USING INDEX sqlite_autoindex_partsupp_1 (ps_partkey=?) / SEARCH supplier USING INTEGER PRIMARY KEY (rowid=?) / SEARCH nation USING INTEGER PRIMARY KEY (rowid=?) / SEARCH region USING INTEGER PRIMARY KEY (rowid=?) / USE SORTER FOR ORDER BY
- q03 B: SCAN orders / SEARCH customer USING INTEGER PRIMARY KEY (rowid=?) / SEARCH lineitem USING INDEX sqlite_autoindex_lineitem_1 (l_orderkey=?) / USE HASH TABLE FOR GROUP BY / USE SORTER FOR ORDER BY
- q03 A: HASH JOIN customer / SEARCH lineitem USING INDEX sqlite_autoindex_lineitem_1 (l_orderkey=?) / SCAN orders / USE HASH TABLE FOR GROUP BY / USE SORTER FOR ORDER BY
- q04 B: SCAN orders / USE HASH TABLE FOR GROUP BY / SEARCH lineitem USING INDEX sqlite_autoindex_lineitem_1 (l_orderkey=?)
- q05 B: MATERIALIZE hash build input for supplier / SCAN orders / SEARCH customer USING INTEGER PRIMARY KEY (rowid=?) / SEARCH lineitem USING INDEX sqlite_autoindex_lineitem_1 (l_orderkey=?) / SEARCH supplier USING COVERING INDEX supplier_nationkey (s_nationkey=?) / SCAN region / HASH JOIN nation / USE HASH TABLE FOR GROUP BY / USE SORTER FOR ORDER BY
- q06 B: SCAN lineitem
- q07 B: SCAN shipping / MATERIALIZE hash build input for lineitem / SCAN nation AS n2 / SEARCH customer USING COVERING INDEX customer_nationkey (c_nationkey=?) / SEARCH orders USING COVERING INDEX orders_custkey (o_custkey=?) / SEARCH lineitem USING INDEX sqlite_autoindex_lineitem_1 (l_orderkey=?) / SCAN nation AS n1 / HASH JOIN supplier / USE HASH TABLE FOR GROUP BY
- q08 B: SCAN all_nations / MATERIALIZE hash build input for customer / SCAN part / SEARCH lineitem USING INDEX lineitem_partkey_suppkey (l_partkey=?) / SEARCH orders USING INTEGER PRIMARY KEY (rowid=?) / SEARCH customer USING INTEGER PRIMARY KEY (rowid=?) / MATERIALIZE hash build input for nation AS n1 / HASH JOIN nation AS n1 / MATERIALIZE hash build input for supplier / HASH JOIN region / SEARCH supplier USING INTEGER PRIMARY KEY (rowid=?) / HASH JOIN nation AS n2 / USE HASH TABLE FOR GROUP BY
- q09 B: SCAN profit / MATERIALIZE hash build input for supplier / SCAN part / SEARCH lineitem USING INDEX lineitem_partkey_suppkey (l_partkey=?) / SEARCH partsupp USING INDEX sqlite_autoindex_partsupp_1 (ps_partkey=? AND ps_suppkey=?) / SEARCH orders USING INTEGER PRIMARY KEY (rowid=?) / SEARCH supplier USING INTEGER PRIMARY KEY (rowid=?) / HASH JOIN nation / USE HASH TABLE FOR GROUP BY
- q10 B: MATERIALIZE hash build input for customer / SCAN lineitem / SEARCH orders USING INTEGER PRIMARY KEY (rowid=?) / SEARCH customer USING INTEGER PRIMARY KEY (rowid=?) / HASH JOIN nation / USE HASH TABLE FOR GROUP BY / USE SORTER FOR ORDER BY
- q10 A: MATERIALIZE hash build input for customer / SCAN orders / SEARCH lineitem USING INDEX sqlite_autoindex_lineitem_1 (l_orderkey=?) / SEARCH customer USING INTEGER PRIMARY KEY (rowid=?) / HASH JOIN nation / USE HASH TABLE FOR GROUP BY / USE SORTER FOR ORDER BY
- q11 B: SCAN nation / SEARCH supplier USING COVERING INDEX supplier_nationkey (s_nationkey=?) / SEARCH partsupp USING INDEX partsupp_suppkey (ps_suppkey=?) / USE HASH TABLE FOR GROUP BY / SCALAR SUBQUERY 1 / SCAN nation / SEARCH supplier USING COVERING INDEX supplier_nationkey (s_nationkey=?) / SEARCH partsupp USING INDEX partsupp_suppkey (ps_suppkey=?) / USE SORTER FOR ORDER BY
- q12 B: SCAN lineitem / SEARCH orders USING INTEGER PRIMARY KEY (rowid=?) / USE HASH TABLE FOR GROUP BY
- q13 B: SCAN c_orders / HASH JOIN orders / SCAN customer / USE HASH TABLE FOR GROUP BY / USE HASH TABLE FOR GROUP BY / USE SORTER FOR ORDER BY
- q14 B: SCAN lineitem / SEARCH part USING INTEGER PRIMARY KEY (rowid=?)
- q15 B: SCALAR SUBQUERY 1 / SCAN revenue0 / SCAN lineitem / USE HASH TABLE FOR GROUP BY / SCAN lineitem / USE HASH TABLE FOR GROUP BY / SCAN revenue0 / SEARCH supplier USING INTEGER PRIMARY KEY (rowid=?) / USE SORTER FOR ORDER BY
- q16 B: LIST SUBQUERY 1 / SCAN supplier / SCAN part / SEARCH partsupp USING COVERING INDEX sqlite_autoindex_partsupp_1 (ps_partkey=?) / USE SORTER FOR GROUP BY / USE HASH TABLE FOR count(DISTINCT) / USE SORTER FOR ORDER BY
- q17 B: SCAN part / SEARCH lineitem USING INDEX lineitem_partkey_suppkey (l_partkey=?) / CORRELATED SCALAR SUBQUERY 1 / SEARCH lineitem USING INDEX lineitem_partkey_suppkey (l_partkey=?)
- q18 B: LIST SUBQUERY 1 / SCAN lineitem / USE HASH TABLE FOR GROUP BY / HASH JOIN customer / SEARCH lineitem USING INDEX sqlite_autoindex_lineitem_1 (l_orderkey=?) / SCAN orders / USE HASH TABLE FOR GROUP BY
- q19 B: SCAN part / SEARCH lineitem USING INDEX lineitem_partkey_suppkey (l_partkey=?)
- q20 B: LIST SUBQUERY 1 / LIST SUBQUERY 2 / SCAN part / SCAN partsupp / SEARCH lineitem USING INDEX lineitem_partkey_suppkey (l_partkey=? AND l_suppkey=?) LEFT-JOIN / SCAN supplier / SEARCH nation USING INTEGER PRIMARY KEY (rowid=?) / USE SORTER FOR ORDER BY
- q20 A: LIST SUBQUERY 1 / LIST SUBQUERY 2 / SCAN part / SCAN partsupp / SEARCH lineitem USING INDEX lineitem_partkey_suppkey (l_partkey=? AND l_suppkey=?) LEFT-JOIN / SCAN nation / SEARCH supplier USING INDEX supplier_nationkey (s_nationkey=?) / USE SORTER FOR ORDER BY
- q21 B: SCAN nation / SEARCH supplier USING INDEX supplier_nationkey (s_nationkey=?) / SEARCH l1 USING INDEX lineitem_suppkey (l_suppkey=?) / SEARCH orders USING INTEGER PRIMARY KEY (rowid=?) / USE HASH TABLE FOR GROUP BY / SEARCH l2 USING INDEX sqlite_autoindex_lineitem_1 (l_orderkey=?) / SEARCH l3 USING INDEX sqlite_autoindex_lineitem_1 (l_orderkey=?) / USE SORTER FOR ORDER BY
- q22 B: SCAN custsale / SCALAR SUBQUERY 1 / SCAN customer / SCAN customer / SEARCH orders USING INDEX orders_custkey (o_custkey=?) / USE HASH TABLE FOR GROUP BY
