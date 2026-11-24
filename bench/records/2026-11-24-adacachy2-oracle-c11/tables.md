| run | oracles | databases | checks | findings | more of the same | seconds |
|---|---|---:|---:|---:|---:|---:|
| main-8001 | diff | 28,674 | 1,171,028 | 292 | 0 | 760 |
| main-8002 | diff | 28,756 | 1,174,648 | 266 | 0 | 760 |
| main-8003 | diff, dqp, eet, norec, pqs, tlp | 27,114 | 1,055,756 | 178 | 1,733 | 760 |
| main-8004 | diff, dqp, eet, norec, pqs, tlp | 27,135 | 1,057,942 | 171 | 1,708 | 760 |
| main-8005 | diff | 30,007 | 1,225,206 | 314 | 0 | 760 |
| main-8006 | diff, dqp, eet, norec, pqs, tlp | 28,174 | 1,098,100 | 190 | 1,754 | 760 |

TOTAL databases 169,860 checks 6,782,680

| oracle | checks | passed | of them, digits alone | failed (before dedupe) | skipped | refused | other plan |
|---|---:|---:|---:|---:|---:|---:|---:|
| diff | 4,030,954 | 2,983,743 | 237 | 640 | 903,169 | 143,402 | 0 |
| dqp | 526,461 | 373,853 | 0 | 19 | 152,589 | 0 | 109,193 |
| eet | 547,278 | 391,700 | 0 | 1,248 | 154,330 | 0 | 0 |
| norec | 547,263 | 484,253 | 0 | 2,826 | 60,184 | 0 | 0 |
| pqs | 414,159 | 382,881 | 0 | 598 | 30,680 | 0 | 0 |
| same | 169,332 | 168,412 | 66 | 101 | 491 | 328 | 0 |
| tlp | 547,233 | 464,346 | 0 | 647 | 82,240 | 0 | 0 |

| oracle | skipped because | count |
|---|---|---:|
| diff | a bare column: its row is any of its group's | 454,498 |
| diff | sqlite refused | 448,209 |
| diff | over the instructions a query may take on SQLite | 308 |
| diff | SQLite's answer holds ill-formed UTF-8 (garbage in, garbage out) | 135 |
| diff | a REAL's text digits, inside a function | 19 |
| dqp | amphora refused the base | 74,345 |
| dqp | a bare column: its row is any of its group's | 59,297 |
| dqp | the index can't serve the query | 18,134 |
| dqp | a derived query meets a value's error (evaluation order) | 773 |
| dqp | over the instructions a query may take on SQLite | 40 |
| eet | amphora refused the base | 74,515 |
| eet | a bare column: its row is any of its group's | 74,036 |
| eet | a derived query errors on SQLite too | 5,051 |
| eet | a derived query meets a value's error (evaluation order) | 515 |
| eet | a derived query reads a table no longer in its FROM | 185 |
| eet | over the instructions a query may take on SQLite | 28 |
| norec | amphora refused both | 55,191 |
| norec | a derived query meets a value's error (evaluation order) | 4,967 |
| norec | a derived query errors on SQLite too | 25 |
| norec | over the instructions a query may take on SQLite | 1 |
| pqs | sqlite can't evaluate it | 24,637 |
| pqs | sqlite misses its own pivot | 5,609 |
| pqs | a derived query meets a value's error (evaluation order) | 434 |
| same | a derived query meets a value's error (evaluation order) | 340 |
| same | sqlite refused | 145 |
| same | SQLite's rows hold ill-formed UTF-8 (garbage in, garbage out) | 6 |
| tlp | amphora refused the base | 68,471 |
| tlp | a derived query meets a value's error (evaluation order) | 10,573 |
| tlp | a REAL sum's partitions round apart from it | 2,840 |
| tlp | a derived query reads a table no longer in its FROM | 319 |
| tlp | a derived query errors on SQLite too | 22 |
| tlp | a partition's sum overflows | 15 |

| feature | databases | checks | passed | failed | skipped | refused | findings: wrong | error | panic |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| (none of them) | 0 | 0 | 0 | 0 | 0 | 0 | 44 | 0 | 527 |
| derived | 0 | 1,535,383 | 1,067,600 | 1,054 | 404,539 | 62,190 | 89 | 0 | 2 |
| except | 0 | 419,414 | 228,248 | 165 | 153,609 | 37,392 | 16 | 0 | 0 |
| intersect | 0 | 418,636 | 228,665 | 154 | 151,783 | 38,034 | 4 | 0 | 0 |
| natural | 0 | 414,903 | 291,707 | 310 | 107,093 | 15,793 | 7 | 0 | 0 |
| recursive | 0 | 527,636 | 391,587 | 344 | 120,763 | 14,942 | 199 | 0 | 0 |
| row value | 0 | 434,228 | 277,568 | 4,967 | 123,425 | 28,268 | 320 | 3 | 1 |
| star | 0 | 149,938 | 128,602 | 22 | 20,037 | 1,277 | 6 | 0 | 0 |
| strict | 39,775 | 1,298,822 | 877,050 | 1,067 | 392,559 | 28,146 | 195 | 4 | 19 |
| trigger | 57,788 | 57,787 | 57,430 | 78 | 179 | 100 | 93 | 0 | 0 |
| union | 0 | 874,982 | 516,260 | 310 | 300,309 | 58,103 | 36 | 0 | 0 |
| union all | 0 | 802,486 | 506,579 | 487 | 249,706 | 45,714 | 211 | 0 | 0 |
| upsert | 94,534 | 94,534 | 93,972 | 75 | 308 | 179 | 31 | 0 | 0 |
| using | 0 | 413,716 | 289,633 | 319 | 108,117 | 15,647 | 9 | 0 | 2 |
| view | 59,253 | 1,016,399 | 734,682 | 816 | 245,812 | 35,089 | 48 | 14 | 0 |
| window | 0 | 1,341,305 | 935,585 | 1,087 | 356,577 | 48,056 | 59 | 2 | 88 |
| with | 0 | 732,065 | 526,673 | 283 | 181,179 | 23,930 | 35 | 0 | 0 |
| without rowid | 39,216 | 1,227,618 | 801,472 | 1,067 | 392,906 | 32,173 | 120 | 4 | 1 |

REFUSALS (not a value's error)
| `Parse error: #st ORDER BY term does not match any column in the result set` | 62,216 | union 33412, derived 26931, union all 23769, window 21255 |
| `Parse error: #nd ORDER BY term does not match any column in the result set` | 17,634 | union 9445, window 7827, derived 7629, union all 6761 |
| `Parse error: insufficient registers allocated for expression vector write (start=#, count=#, next_free=#)` | 14,536 | row value 12733, derived 5141, window 4214, view 3433 |
| `Internal error: secondary index lacks primary key locator` | 4,616 | without rowid 4498, derived 1341, union 1013, window 932 |
| `Parse error: #rd ORDER BY term does not match any column in the result set` | 3,668 | window 2265, union 1987, derived 1586, union all 1382 |
| `Parse error: no such table: w#` | 1,293 | with 959, recursive 701, union all 639, union 563 |
| `Parse error: #th ORDER BY term does not match any column in the result set` | 422 | window 422, union 230, derived 177, union all 143 |
| `Parse error: row value misused` | 224 | row value 189, derived 74, view 69, strict 48 |
| `Parse error: parenthesized conditional should have exactly one expression` | 11 | row value 11, window 11, derived 6, without rowid 3 |
| `Parse error: table not found` | 4 | window 4, recursive 2, with 2, strict 1 |
value's errors 39106 messages 124

SETUP REFUSED
| `Parse error: INSTEAD OF triggers are not supported yet` | 12,547 | `CREATE TRIGGER o0 INSTEAD OF INSERT ON v0 BEGIN INSERT INTO lg VALUES ('v0', new.c0, new.c1); END` |
| `near "STRICT": syntax error` | 7,701 | `CREATE TABLE t0(c0 TEXT PRIMARY KEY, c1 REAL) STRICT, WITHOUT ROWID` |
| `NOT NULL constraint failed: t#.c# (#)` | 4,045 | `INSERT OR IGNORE INTO t0 VALUES (-9223372036854775808, 0, NULL)` |
| `Parse error: triggered mutations of WITHOUT ROWID tables are not supported` | 2,423 | `INSERT OR REPLACE INTO t1 VALUES (1.5, 9223372036854775807, NULL), (9223372036854775807, 1e308, 9223372036854775807), (x'c3bf', 'A', 10)` |
| `Parse error: insufficient registers allocated for expression vector write (start=#, count=#, next_free=#)` | 219 | `INSERT INTO t1 VALUES (3, 3.0, '1.5') ON CONFLICT DO UPDATE SET c2 = (t1.c0 COLLATE RTRIM), c2 = t1.c1 WHERE ((0, 255, t1.c0) BETWEEN (255, t1.c1, t1.c0) AND (excluded.c1` |
| `cannot store TEXT value in BLOB column t#.c# (#)` | 24 | `INSERT OR IGNORE INTO t0 VALUES (x'', 10, x'00'), (x'c3bf', NULL, x'00'), ('A', 7, NULL)` |
| `cannot store TEXT value in INT column t#.c# (#)` | 18 | `INSERT OR IGNORE INTO t1 VALUES ('ABC', x'41', 9223372036854775807, NULL)` |
| `cannot store INT value in BLOB column t#.c# (#)` | 16 | `INSERT OR IGNORE INTO t0 VALUES (-1, NULL, 3), (NULL, 2.0, 7)` |
| `cannot store REAL value in INTEGER column t#.c# (#)` | 15 | `INSERT OR IGNORE INTO t1 VALUES (-1e-308, 10, 7, -100)` |
| `cannot store TEXT value in REAL column t#.c# (#)` | 15 | `INSERT OR IGNORE INTO t0 VALUES (NULL, '10', 'x'), (-1, -100, 3)` |
| `datatype mismatch` | 14 | `INSERT INTO t0 VALUES (1), (3) ON CONFLICT(c0) DO UPDATE SET c0 = (excluded.c0 COLLATE NOCASE)` |
| `cannot store REAL value in BLOB column t#.c# (#)` | 13 | `INSERT OR IGNORE INTO t0 VALUES ('A', NULL, 1000000000000000.0, NULL), ('10', x'41', x'00', 'A'), ('x', x'', x'00', 'true')` |
rest 66
