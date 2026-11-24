| run | oracles | databases | checks | findings | more of the same | seconds |
|---|---|---:|---:|---:|---:|---:|
| main-27001 | diff | 16,620 | 681,391 | 290 | 0 | 620 |
| main-27002 | diff | 16,726 | 685,741 | 268 | 0 | 620 |
| main-27003 | diff | 17,592 | 721,246 | 280 | 0 | 620 |
| main-27004 | diff, dqp, eet, norec, pqs, tlp | 24,438 | 956,152 | 71 | 1,366 | 620 |
| main-27005 | diff, dqp, eet, norec, pqs, tlp | 23,636 | 924,602 | 82 | 1,304 | 621 |
| main-27006 | diff, dqp, eet, norec, pqs, tlp | 25,511 | 998,157 | 74 | 1,425 | 620 |
| main-27007 | diff, dqp, eet, norec, pqs, tlp | 23,255 | 910,535 | 81 | 1,307 | 623 |
| main-27008 | cert, diff, dqp, eet, norec, order, pqs, tlp | 9,325 | 368,897 | 85 | 378 | 620 |
| main-27009 | cert, diff, dqp, eet, norec, order, pqs, tlp | 8,683 | 343,841 | 104 | 402 | 620 |
| main-27010 | diff | 16,763 | 687,280 | 281 | 0 | 620 |

TOTAL databases 182,549 checks 7,277,842 findings 1616 (unique 1581)

| oracle | checks | passed | of them, digits alone | failed (before dedupe) | skipped | refused | other plan |
|---|---:|---:|---:|---:|---:|---:|---:|
| cert | 90,038 | 89,531 | 0 | 83 | 10 | 414 | 0 |
| diff | 3,443,577 | 2,713,515 | 236 | 1,451 | 683,636 | 44,975 | 0 |
| dqp | 707,845 | 537,243 | 0 | 0 | 170,602 | 0 | 157,310 |
| eet | 735,620 | 563,247 | 0 | 1,100 | 171,273 | 0 | 0 |
| norec | 735,616 | 674,907 | 0 | 3,488 | 57,221 | 0 | 0 |
| order | 90,038 | 89,601 | 0 | 65 | 372 | 0 | 0 |
| pqs | 556,943 | 516,051 | 0 | 731 | 40,161 | 0 | 0 |
| same | 182,547 | 181,692 | 75 | 0 | 546 | 309 | 0 |
| tlp | 735,618 | 645,613 | 0 | 880 | 89,125 | 0 | 0 |

| oracle | skipped because | count |
|---|---|---:|
| cert | sqlite refused | 10 |
| diff | a bare column: its row is any of its group's | 387,249 |
| diff | sqlite refused | 295,971 |
| diff | over the instructions a query may take on SQLite | 273 |
| diff | SQLite's answer holds ill-formed UTF-8 (garbage in, garbage out) | 119 |
| diff | a REAL's text digits, inside a function | 24 |
| dqp | a bare column: its row is any of its group's | 80,129 |
| dqp | amphora refused the base | 63,560 |
| dqp | the index can't serve the query | 25,791 |
| dqp | a derived query meets a value's error (evaluation order) | 1,051 |
| dqp | over the instructions a query may take on SQLite | 71 |
| eet | a bare column: its row is any of its group's | 99,094 |
| eet | amphora refused the base | 64,151 |
| eet | a derived query errors on SQLite too | 6,972 |
| eet | a derived query meets a value's error (evaluation order) | 1,001 |
| eet | over the instructions a query may take on SQLite | 55 |
| norec | amphora refused both | 50,593 |
| norec | a derived query meets a value's error (evaluation order) | 6,606 |
| norec | a derived query errors on SQLite too | 21 |
| norec | over the instructions a query may take on SQLite | 1 |
| order | amphora refused the base | 372 |
| pqs | sqlite can't evaluate it | 31,818 |
| pqs | sqlite misses its own pivot | 7,721 |
| pqs | a derived query meets a value's error (evaluation order) | 622 |
| same | a derived query meets a value's error (evaluation order) | 377 |
| same | sqlite refused | 164 |
| same | SQLite's rows hold ill-formed UTF-8 (garbage in, garbage out) | 5 |
| tlp | amphora refused the base | 69,823 |
| tlp | a derived query meets a value's error (evaluation order) | 15,323 |
| tlp | a REAL sum's partitions round apart from it | 3,916 |
| tlp | a derived query errors on SQLite too | 38 |
| tlp | a partition's sum overflows | 23 |
| tlp | over the instructions a query may take on SQLite | 2 |

| feature | databases | checks | passed | failed | skipped | refused | findings: wrong | error | estimate | panic |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| (none of them) | 0 | 0 | 0 | 0 | 0 | 0 | 14 | 2 | 80 | 0 |
| derived | 0 | 1,563,742 | 1,192,437 | 1,915 | 349,710 | 19,680 | 489 | 4 | 0 | 0 |
| except | 0 | 406,781 | 264,473 | 505 | 130,942 | 10,861 | 220 | 0 | 0 | 0 |
| intersect | 0 | 407,608 | 266,519 | 322 | 129,733 | 11,034 | 66 | 0 | 0 | 0 |
| natural | 0 | 418,563 | 322,823 | 542 | 91,133 | 4,065 | 46 | 0 | 0 | 0 |
| recursive | 0 | 503,444 | 396,107 | 273 | 102,730 | 4,334 | 113 | 0 | 0 | 0 |
| row value | 0 | 466,748 | 329,946 | 6,323 | 112,939 | 17,540 | 29 | 12 | 0 | 0 |
| star | 0 | 141,868 | 123,393 | 1 | 17,492 | 982 | 0 | 0 | 0 | 0 |
| strict | 49,496 | 1,391,927 | 1,142,750 | 1,542 | 237,853 | 9,782 | 287 | 5 | 36 | 0 |
| trigger | 64,159 | 64,159 | 63,856 | 0 | 197 | 106 | 18 | 0 | 6 | 0 |
| union | 0 | 843,358 | 575,045 | 1,463 | 255,993 | 10,857 | 1195 | 0 | 0 | 0 |
| union all | 0 | 778,974 | 554,608 | 777 | 212,994 | 10,595 | 259 | 0 | 0 | 0 |
| upsert | 104,352 | 104,351 | 103,820 | 0 | 358 | 173 | 1 | 0 | 1 | 0 |
| using | 0 | 419,045 | 321,987 | 515 | 92,636 | 3,907 | 42 | 1 | 0 | 0 |
| view | 63,766 | 1,049,582 | 820,336 | 1,207 | 215,867 | 12,172 | 275 | 15 | 0 | 0 |
| window | 0 | 1,297,904 | 975,685 | 1,110 | 307,340 | 13,769 | 286 | 1 | 0 | 0 |
| with | 0 | 699,221 | 536,214 | 416 | 153,479 | 9,112 | 170 | 0 | 0 | 0 |
| without rowid | 49,426 | 1,310,707 | 1,063,209 | 1,421 | 236,166 | 9,911 | 299 | 5 | 28 | 0 |

REFUSALS (not a value's error)
| `Parse error: insufficient registers allocated for expression vector write (start=#, count=#, next_free=#)` | 13,945 | row value 12329, derived 4782, window 4059, view 3201 |
| `Parse error: row value misused` | 247 | row value 226, derived 82, view 62, union 59 |
| `Parse error: incomplete infinity` | 2 | derived 2, window 2 |
| `Parse error: Expected true` | 1 | window 1, with 1 |
value's errors 31,503 messages 100

VALUE ERRORS
| `Parse error: malformed JSON` | 7,565 |
| `json_object() requires an even number of arguments` | 2,833 |
| `JSON path error near: "#"` | 2,693 |
| `Parse error: Unexpected character` | 2,184 |
| `json_set() needs an odd number of arguments` | 1,471 |
| `json_array_insert() needs an odd number of arguments` | 1,467 |
| `JSON cannot hold BLOB values` | 1,379 |
| `json_insert() needs an odd number of arguments` | 1,369 |
| `json_replace() needs an odd number of arguments` | 1,338 |
| `JSON path error near: "#.#"` | 1,220 |
| `json_object() labels must be TEXT` | 1,127 |
| `FLAGS parameter to json_valid() must be between # and #` | 846 |
| `Parse error: Unexpected end of input` | 712 |
| `JSON path error near: "-#"` | 632 |
| `ESCAPE expression must be a single character` | 499 |
| `Parse error: bad JSON path: '#'` | 472 |
| `Parse error: bad JSON path: ''` | 438 |
| `string or blob too big` | 400 |
| `JSON path error near: "-#.#"` | 229 |
| `Parse error: bad JSON path: 'A'` | 198 |
| `Parse error: bad JSON path: '#.#'` | 197 |
| `JSON path error near: "#.#e+#"` | 152 |
| `integer overflow` | 148 |
| `JSON path error near: "-#.#e-#"` | 140 |
| `Parse error: bad JSON path: 'ÿ'` | 125 |
| `Parse error: bad JSON path: '-#'` | 124 |
| `Parse error: bad JSON path: ''` | 121 |
| `Parse error: bad JSON path: 'true'` | 110 |
| `Parse error: bad JSON path: '_'` | 105 |
| `Parse error: bad JSON path: '%'` | 97 |
| `Parse error: bad JSON path: 'é'` | 95 |
| `Parse error: bad JSON path: 'ABC'` | 94 |
| `Parse error: bad JSON path: 'a '` | 93 |
| `Parse error: bad JSON path: ' a'` | 84 |
| `Parse error: bad JSON path: 'x'` | 82 |
| `Parse error: bad JSON path: 'a'` | 81 |
| `Parse error: bad JSON path: 'abc'` | 81 |
| `Parse error: bad JSON path: ' '` | 80 |
| `Parse error: bad JSON path: '#x#'` | 77 |
| `Parse error: bad JSON path: '#e#'` | 63 |
| `Parse error: bad JSON path: 'g#'` | 51 |
| `JSON path error near: "-#.#e+#"` | 39 |
| `Parse error: bad JSON path: '#.#e+#'` | 19 |
| `Parse error: bad JSON path: '-#.#'` | 19 |
| `Parse error: bad JSON path: '-#.#e-#'` | 19 |
| `Parse error: bad JSON path: '[#,#,#]'` | 15 |
| `Parse error: bad JSON path: '{"a":#,"b":[#,#]}'` | 14 |
| `Parse error: bad JSON path: '{"a":null}'` | 14 |
| `Parse error: bad JSON path: 'NULL'` | 10 |
| `JSON path error near: "Inf"` | 6 |
| `Parse error: bad JSON path: 'integer'` | 6 |
| `Parse error: bad JSON path: 'text'` | 4 |
| `JSON path error near: "#.#e-#"` | 3 |
| `Parse error: bad JSON path: ''` | 3 |
| `Parse error: bad JSON path: '#'` | 3 |
| `Parse error: bad JSON path: 'ÿ#'` | 3 |
| `Parse error: bad JSON path: 'ABC-#.#'` | 3 |
| `Parse error: bad JSON path: '#'` | 2 |
| `Parse error: bad JSON path: '#.#ÿ'` | 2 |
| `Parse error: bad JSON path: '-#.#e+#'` | 2 |
| `Parse error: bad JSON path: '#.#.#'` | 2 |
| `Parse error: bad JSON path: '''#'''` | 2 |
| `Parse error: bad JSON path: '''#.#'''` | 2 |
| `Parse error: bad JSON path: 'real'` | 2 |
| `Parse error: bad JSON path: 'null'` | 2 |
| `Parse error: bad JSON path: '""'` | 1 |
| `Parse error: bad JSON path: '#.#%'` | 1 |
| `Parse error: bad JSON path: '#.#e+#A'` | 1 |
| `Parse error: bad JSON path: '''[#,#,#]'''` | 1 |
| `Parse error: bad JSON path: 'xx'` | 1 |
| `Parse error: bad JSON path: '#:#:#'` | 1 |
| `Parse error: bad JSON path: ''' '''` | 1 |
| `Parse error: bad JSON path: '''%'''` | 1 |
| `Parse error: bad JSON path: ''''''` | 1 |
| `Parse error: bad JSON path: '-#-#-#'` | 1 |
| `Parse error: bad JSON path: 'ABC#'` | 1 |
| `Parse error: bad JSON path: '"#"'` | 1 |
| `Parse error: bad JSON path: '#-#'` | 1 |
| `Parse error: bad JSON path: '''#e#'''` | 1 |
| `Parse error: bad JSON path: '''ABC'''` | 1 |
| `Parse error: bad JSON path: '''integer'''` | 1 |
| `Parse error: bad JSON path: '+A'` | 1 |
| `Parse error: bad JSON path: 'X'''''` | 1 |
| `Parse error: bad JSON path: '["#","#"," "]'` | 1 |
| `Parse error: bad JSON path: '{#Gtrue'` | 1 |
| `Parse error: bad JSON path: '#x#.#'` | 1 |
| `Parse error: bad JSON path: '-#-#-# #:#:#.#'` | 1 |
| `Parse error: bad JSON path: '["a ","#.#",#]'` | 1 |
| `Parse error: bad JSON path: '[#.#,#.#]'` | 1 |
| `Parse error: bad JSON path: '[-#.#,null,"#x#"]'` | 1 |
| `Parse error: bad JSON path: 'C-#'` | 1 |
| `Parse error: bad JSON path: 'true'` | 1 |
| `Parse error: bad JSON path: '�-#'` | 1 |
| `Parse error: bad JSON path: ''` | 1 |
| `Parse error: bad JSON path: '#.#x'` | 1 |
| `Parse error: bad JSON path: '''-#'''` | 1 |
| `Parse error: bad JSON path: '-#a '` | 1 |
| `Parse error: bad JSON path: 'A#'` | 1 |
| `Parse error: bad JSON path: '[#,-#.#e+#]'` | 1 |
| `Parse error: bad JSON path: '�'` | 1 |
| `Parse error: malformed JSON` | 7,565 |
| `json_object() requires an even number of arguments` | 2,833 |
| `JSON path error near: "#"` | 2,693 |
| `Parse error: Unexpected character` | 2,184 |
| `json_set() needs an odd number of arguments` | 1,471 |
| `json_array_insert() needs an odd number of arguments` | 1,467 |
| `JSON cannot hold BLOB values` | 1,379 |
| `json_insert() needs an odd number of arguments` | 1,369 |

SETUP REFUSED
| `Parse error: INSTEAD OF triggers are not supported yet` | 13,494 | `CREATE TRIGGER o0 INSTEAD OF INSERT ON v0 BEGIN INSERT INTO lg VALUES ('v0', new.c0, new.c2); END` |
| `Parse error: insufficient registers allocated for expression vector write (start=#, count=#, next_free=#)` | 213 | `INSERT INTO t0 VALUES ('a ') ON CONFLICT DO UPDATE SET c0 = (CASE ' a' WHEN NULL THEN t0.c0 END) WHERE ((t0.c0, excluded.c0, excluded.c0) NOT BETWEEN (7, t0.c0, -1) AND (excluded.c0, x'c3bf'` |
| `cannot store TEXT value in BLOB column t#.c# (#)` | 9 | `INSERT OR IGNORE INTO t1 VALUES ('1.5', 100), (x'00', 1)` |
| `cannot store BLOB value in REAL column t#.c# (#)` | 8 | `INSERT OR IGNORE INTO t0 VALUES (9007199254740993, -2.5, 100), (5, 100, x'0102')` |
| `cannot store TEXT value in INTEGER column t#.c# (#)` | 8 | `INSERT INTO t0 VALUES (255, 1, NULL, 255), (100, 2, 0, 9007199254740993), (2, 5, ' ', NULL) ON CONFLICT(c1) DO UPDATE SET c3 = (NOT (9223372036854775807 != t0.c2))` |
| `cannot store TEXT value in INT column t#.c# (#)` | 8 | `INSERT OR IGNORE INTO t0 VALUES ('abc', NULL, 255), (9223372036854775807, x'00', 7), (2, x'', -1)` |
| `cannot store REAL value in INT column t#.c# (#)` | 6 | `INSERT OR IGNORE INTO t0 VALUES (-2.5, 10, 9007199254740993), (10, -1, -9223372036854775808), (255, 3, 5)` |
| `cannot store REAL value in BLOB column t#.c# (#)` | 6 | `INSERT OR IGNORE INTO t0 VALUES (NULL, 9007199254740993), (-2.5, 9007199254740993), (x'c3bf', NULL)` |
| `cannot store REAL value in INTEGER column t#.c# (#)` | 5 | `INSERT INTO t0 VALUES (2, -1), (0.5, -1), (7, 100) ON CONFLICT DO NOTHING` |
| `cannot store INT value in BLOB column t#.c# (#)` | 4 | `INSERT OR IGNORE INTO t0 VALUES (x'', x'00', 9007199254740993, x'0102'), (-100, NULL, 100, NULL)` |
| `cannot store BLOB value in TEXT column t#.c# (#)` | 3 | `INSERT OR IGNORE INTO t0 VALUES (-1, x'c3bf', -1), ('', '0', 1)` |
| `cannot store BLOB value in INT column t#.c# (#)` | 1 | `INSERT OR IGNORE INTO t0 VALUES (1, '1', 7, -100), (1, ' a', x'c3bf', -100)` |
| `cannot store BLOB value in INTEGER column t#.c# (#)` | 1 | `INSERT OR IGNORE INTO t2 VALUES (5, x'00', x'c3bf'), (0, 2, NULL), (-9223372036854775808, 1, NULL)` |
rest 0
