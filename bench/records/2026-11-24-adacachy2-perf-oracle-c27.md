# Lane C27: the wide oracle on main of 2026-11-24, its findings ranked, and the worst one fixed (2026-11-24, adacachy2)

Lane's hour: 12:13:05Z to 13:13:05Z. Losses first.

- **Wrong data is left on main: ED**, a BETWEEN in an UPSERT's DO UPDATE compares the table's column without its collation, so a row SQLite updates is not updated. It came up in the seeds run after CY's landing (below), and is on
  988f0fb4 too; by the ranking's order it is worse than CY, which was the worst of the first run. Not traced, not fixed.
- **CY's landing (ed7007d4, 12:48Z) broke a form that answered, and the fix forward is not on main**: an arm with a window function, whose columns Amphora reads out of a layer of its own, counts as a column:
  `SELECT '', count(*), count(*) OVER () FROM t GROUP BY '' UNION SELECT 'z', a, 2 FROM t` (a NOCASE) gives 3 rows for 2 on main. The fix is perf/oracle-c27 776a8b86. As 3055995b (the same but for one closure and main's tip) its
  tests passed on Linux (compound_collations 8,744 answers, 0 differ; oracle_findings 102; window_columns 9; cte_scopes 1) and the release build answers the finding; clippy had one error there (`or_fun_call`, changed in 776a8b86), check-production ended
  on it. No check ran on 776a8b86: adacachy2 and the Macs stopped answering at 13:01Z (runs r2267, r2268 and r2275 queued). The sweep as landed first had no arm of two columns; the 597 `windowed` queries are in that commit.
- **One cause is fixed, CY, and five wrong answers are left on main**: DW, DU, DV and DX (JSONB's bytes), new to the oracle, and EB, met by hand beside the fix. Each is in
  plain SQL at default settings. Their places are named by reading alone, DX's not at all, and none was changed. DW is the worst left.
- **84% of the findings are SQLite's own** (RX, 1,334 of 1,581). Since lane C19 Amphora takes `ORDER BY (1 COLLATE x)` over a
  compound, and SQLite's rows there change with the ORDER BY. The reducer cuts each down for up to 3 s, so at most two thirds of the
  6,204 process-seconds went to them (inferred). The oracle has no rule for it: the next run meets it again.
- **C11's reading of CY was wrong, and a decision rested on it** (LANES, 09:41Z). The sentence of lang_select.html was given "from memory"; as read
  today it ends "except that greater precedence is not assigned to a collation sequence specified with the postfix COLLATE
  operator". So Amphora's answers C11 kept (a COLLATE on the right) were wrong by the page, and are SQLite's now.
- **The fix answers one thing otherwise than SQLite, which it answered alike before**: `SELECT id FROM p UNION SELECT a FROM t`, id an INTEGER
  PRIMARY KEY, a NOCASE: SQLite's rowid has no collation; by the page a column that declares none has BINARY. 2 rows for SQLite's 1 on 'Q' and 'q'.
- **The sweep leaves out** what SQLite does otherwise than the page: an EXCEPT or INTERSECT of two arms with no collation before one that has
  (six queries hold the page's answer, three of them SQLite's otherwise; the test's commit message says five, and 46 kinds of arm where there are 56), a UNION of four arms or more (SQLite merges them two by two), a VALUES of several rows as a first arm.
- **EB is left beside the fix**: an EXCEPT in an IN's list gives its rows to the IN's index without the IN's affinity (`SELECT n, n IN (SELECT s FROM tx EXCEPT SELECT 'z') FROM i`,
  n INTEGER 1, s TEXT '1': SQLite 1, Amphora NULL, on main before and after). It is in the function the fix changed and is its own cause; no oracle makes it, so it is named and not fixed.
- **Causes are named by rules over the replays, 9 of the 1,581 by hand** (`sort27.py`); the smallest of each was read. RX is told by SQLite
  disagreeing with itself without its ORDER BY, AH by the system's build; each of RW's 18 was read: SQLite gives one group twice (14), or another order with its
  optimizations off (4). No code of SQLite's was read for any of them.
- **Not analysed**: the 137 estimates (CERT 83, order 54; every one still fails on main by `--replay-check`). EA's 59 statements: one read.
- **Suites**: skip_scan's `a_skip_scan_reads_what_a_scan_reads` fails on main too (5 of 5 runs, 3 on the branch and 2 on main's code, the same
  line). Not run: the engine's whole lib suite (27 lib tests by name), `crates/amphora`, the corpus, macOS.
- A quarter of the differential check is not compared: `a bare column` 387,249 and `sqlite refused` 295,971 of 3,443,577 (C11's were
  454,498 and 448,209 of 4,030,954).

## What ran

- Engine: main 988f0fb4, its tip at 12:13Z (988f0fb4e696588554002fae2c7ec17daadf4525). Reference: SQLite 3.54.0, `bench/oracles/sqlite.sh`; the system's
  SQLite 3.54.0 as well, to tell AH.
- The oracles' module alone on the engine (`oracle-lite`, lane C8's), release, the bench's features with `io_uring`, the database opened as production
  opens it, overflow checks on as the workspace's release profile has them (C11's had none). 64 s to build with 8 jobs.
- `oracle-lite --oracle --oracles L --seconds 620 --seed S --reduce-seconds 3 --features bench/corpus/features.json --sqlite LIB`: ten processes, one core each
  (`nice -n 10`, two runs of `needs cpus=5,mem=12g`), 40 checks a database, under `timeout 760`. adacachy2 shared, 1-minute load 20 to 42 on 32 cores. Nothing was timed.
- Seeds 27001 to 27010, none of them used before.

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

**182,549 databases, 7,277,842 checks; 1,581 findings (35 of them met in two runs).** Every run ended by its own clock. **No panic and no
hang**; two checks took over a second (2.3 s, seed 27003 state 11869; 1.1 s, seed 27006 state 22949).

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

The `same` check failed nowhere (C11: 101): no database's statements left other rows than SQLite's. DQP failed nowhere (C11: 19). TLP,
NoREC, EET and PQS found errors alone, 13 of them once deduped. `digits alone`: as C11's record.

### Skips, by reason

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

### Findings per feature

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

Counts of findings here include the reference's own (RX, RW, AH) and the estimates.

## The ranking

Each finding is in its reduced form, replayed at default, parallel, statistics and both against SQLite 3.54.0. Every wrong answer and error differs at all four.

| rank | cause | findings | which oracles | plain SQL, SQLite / Amphora (main) | new? |
|---:|---|---:|---|---|---|
| - | panics, hangs | 0 | | | |
| - | wrong data (`same`) | 0 in this run | | | |
| 0 | ED, wrong data: a BETWEEN in a DO UPDATE loses the column's collation | 1 in seeds 27021 to 27026 | same | `... ON CONFLICT DO UPDATE SET c1 = excluded.c1 WHERE t0.c0 BETWEEN 'A' AND excluded.c0` ('abc', 'old'; c0 NOCASE): 'new' / 'old' | new, on 988f0fb4 too |
| 1 | CY, a compound's collation (fixed here) | 36, and 2 of a rowid | differential | `SELECT count(*) FROM (SELECT email FROM staff UNION SELECT email FROM guest)`: 6 / 3; EXCEPT 3 / 1 | C11's, 20 |
| 2 | DW, the group's column in a subquery's subquery that reads its parent's row too | 1 | differential | `SELECT q.k, (SELECT count(*) FROM s WHERE EXISTS (SELECT 1 FROM s AS s2 WHERE s2.k = s.k AND s2.k = q.k)) FROM q GROUP BY q.k`: 'a' 1, 'b' 0 / 'a' 0, 'b' 0 | new |
| 3 | DU, min and max of an expression that holds a COLLATE | 1 | differential | `SELECT min(a \|\| '' COLLATE NOCASE) FROM c` ('ABC', 'a '): 'a ' / 'ABC' | new |
| 4 | DV, CAST of text past 64 bits to NUMERIC | 1 | differential | `SELECT CAST('12345678901234567890' AS NUMERIC) = 12345678901234567890`: 1 / 0 | new |
| 5 | DX, JSONB's type of a JSON5 string that holds a double quote | 1 | differential | `SELECT hex(jsonb('''x"y'''))`: 39782279 / 37782279 | new |
| 6 | EB, an EXCEPT in an IN's list without the IN's affinity | 0, by hand | | `SELECT n, n IN (SELECT s FROM tx EXCEPT SELECT 'z') FROM i` (1 and '1'): 1 / NULL | new |
| 7 | CJ, error: a BETWEEN's side computed ahead of the rows | 2 | PQS | `SELECT body FROM ev WHERE NOT EXISTS (SELECT 1 FROM lim WHERE json_extract(ev.body, '$.n') BETWEEN 1 AND lim.hi)`: 1 row / "malformed JSON" | C8's |
| 8 | CW, error: what SQLite pushes into a view or leaves unread | 8 | PQS | `SELECT a FROM dv WHERE id = 1` (dv a DISTINCT view): 1 / "malformed JSON" | C11's, other forms |
| 9 | C11's CM, refused | 2 (13,945 refusals, 213 statements) | NoREC, EET | `SELECT ('a', 1) BETWEEN ('a', 0) AND ('a', 2)`: 1 / "insufficient registers allocated ..." | C11's (C20 has it) |
| 10 | INSTEAD OF triggers, refused | 13,494 statements | setup | `CREATE TRIGGER g INSTEAD OF INSERT ON v ...`: taken / refused | known (C28 has it) |
| 11 | C11's CN, refused | 1 (247 refusals) | TLP | `SELECT t.a FROM t JOIN s ON (t.a, 'z') <> (SELECT max(y), min(x) FROM s AS s2) OR 1`: 1 row / "row value misused" | C11's (C20 has it) |
| 12 | EA, refused: a STRICT row's type checked before its taken rowid | 59 statements | setup | `INSERT OR IGNORE INTO t0 VALUES (1, 'x')` (1 is taken, b BLOB): left out / "cannot store TEXT value in BLOB column t0.b" | new |
| 13 | DY, refused: JSON's Inf, QNaN | 2 refusals | differential | `SELECT json('Inf')`: 9e999 / "malformed JSON" | new |
| - | CI, json_pretty's indent from a REAL | 2 | differential | `SELECT json_pretty('[1]', 100.0)`: indent 100.0 / 100 | C8's |
| by design | CK, a REAL's text digits | 16 | differential | `SELECT octet_length(quote(asinh(1)))`: 19 / 17 | C8's |
| estimates | CL, CERT and the order check | 137 | cert 83, order 54 | no answer is wrong | C8's |
| not Amphora's | RX, SQLite's compound under an ORDER BY with a COLLATE | 1,334 | differential | `SELECT NULL, NULL FROM t UNION SELECT 'A', count(*) FROM t GROUP BY 'A' ORDER BY 1, 2 COLLATE RTRIM` (t empty): 1 row / 0, and SQLite's is 0 without the COLLATE | new to the run |
| not Amphora's | RW, SQLite's window over a GROUP BY | 18 | differential | `SELECT 1, (7 IS c0), NULL, count(*) OVER (ORDER BY NULL) FROM t GROUP BY 1, (7 IS c0)`: one group twice / once | C11's |
| not Amphora's | AH, the reference's LIKE optimization | 18 | differential | the system's SQLite answers as Amphora | known |
| not Amphora's | JP, SQLite's JSON cache | 1 | differential | `SELECT hex(jsonb(json_patch(100, '0x10') \|\| ''))`: 4430783130 / 233136, `jsonb('16')`'s | new |

Where each new cause's answer is made, by reading the code and not run (none of it was changed):

- DW: `translate/main_loop/conditions.rs`, where a subquery that depends on a loop's row is emitted with the query's plain resolver and `preserve_outer_expr_cache` false.
  `emit_non_from_clause_subqueries_for_phase` (subquery.rs) gives one that runs before the loops `fork_with_outer_expr_cache`: the registers the enclosing query's group
  was read into. The loops' subquery reads NULL for the group's column (run: `SELECT q.k, (SELECT group_concat((SELECT quote(q.k) || s.k)) FROM s) FROM q GROUP BY q.k` gives NULLa,NULLc for 'a'a,'a'c). A subquery that reads the group's column alone runs before the loops, and answers.
  A compound subquery that reads the outer row is refused ("compound SELECT queries not supported yet in WHERE clause subqueries", met by hand; the generator makes none). Lane C23's branch 35324c66 was built and run: DL answers, the DW forms do not.
- DU: `translate/aggregation.rs` `agg_arg_collation`, `if let Expr::Collate` on the argument alone.
- DV: `util.rs` `checked_cast_text_to_numeric`: `(value * factor).round() / factor` with `factor = 10^(15 - digits)`, where an integer overflows.

The documents: CY, lang_select.html 3 as read 2026-11-24 12:24Z (quoted above); DU, lang_aggfunc.html ("the first non-NULL value that would appear in an ORDER BY of the column") with
datatype3.html 7.1 ("any subexpression of the operand uses the postfix COLLATE operator"); DX, jsonb.html ("TEXT ... does not contain any escapes nor any characters
that need to be escaped for either SQL or JSON"); DY, json1.html ("SQLite also allows the abbreviation "Inf" ... any combination of upper and lower case letters"). CJ, CW, EA: no page says.

Each cause in plain SQL, both answers, at the four settings: `plain-main.txt` (main), `plain-fix.txt` (with the fix), from `plain29.py` to `plain34.py`.

### RX: Amphora's rows are right where SQLite's change

`selfcheck27.py` ran each of the 1,431 wrong answers on SQLite alone, as it stands, without its outermost ORDER BY and with its optimizations off
(SQLITE_TESTCTRL_OPTIMIZATIONS): 1,334 change without the ORDER BY, 8 with the optimizations off (4 in RW, 1 in CY, 3 in RX). `variants27.py` then compared Amphora's
rows with the ORDER BY to SQLite's without it, and Amphora's without it: alike in 1,333 of the 1,334 at all four settings. In the last, SQLite refuses the
query without the ORDER BY ("aggregate functions are not allowed in the GROUP BY clause") and answers it with. All 1,334 have an ORDER BY term with a COLLATE.

## C11's findings on this main

C11's 862 findings and the 526 more of CP's panic (its run directories are still on adacachy2), replayed on main 988f0fb4 under the four settings.

| C11's cause | findings | answer on main | still differ | with the fix |
|---|---:|---:|---:|---|
| CP, panic | 527 | 527 | 0 | |
| CT, row value IN with a NULL | 323 | 323 | 0 | |
| CU, a recursive CTE's number | 183 | 181 | 2 | 1 keeps another of two equal texts |
| CO, panic | 88 | 88 | 0 | |
| CV | 54 | 54 | 0 | |
| DA, wrong data | 28 | 28 | 0 | |
| CY | 20 | 0 | 20 | 8 answer; 12 keep another of several equal texts |
| CX | 6 | 5 | 1, a random rowid | |
| CZ | 6 | 4 | 2, CY's | answer |
| DC, panic | 3 | 3 | 0 | |
| DB | 2 | 2 | 0 | |
| CQ, CR, CS | 1 each | 1 each | 0 | |
| CW | 12 | 9 | 3 | |
| CM | 2 | 0 | 2 | |
| CN | 1 | 0 | 1 | |
| CK (digits) | 76 | 64 | 12 | |
| DE and 10 others its rules named nothing for | 11 | 3 | 8: digits 3, json_pretty 3, a COLLATE inside quote() in a compound 2 | those 2 keep another of two equal texts |
| RW, AH (not Amphora's) | 15, 26 | 0 | as C11 has them | |
| a random rowid, ill-formed text | 1, 1 | 0 | as C11 has them | |

## CY, traced and fixed

One place. The index a compound's rows are told apart in (`translate/compound_select.rs` `create_dedupe_index`) took each column's collation from the arm
left of the last operator and the one right of it, by `get_collseq_from_expr`, which has a column that declares no collation as having none. So a BINARY column on the left gave
way to a NOCASE one on the right; an arm before the last two was not read; every operator of a chain of UNIONs and EXCEPTs shared the last one's index; an EXCEPT in an IN used the IN's; the
index an ORDER BY's rows are collected in read the first and last arms alone; a recursive CTE (`recursive_cte.rs`) read every arm, but a plain column as none.

By lang_select.html 3 the first arm's that has a collation, a COLLATE or a column (BINARY where it declares none): `collate.rs` `compound_column_collation`, over all of the arms left
of the operator and the one right of it. Two operators share an index only where they compare alike (`tells_rows_apart_by`); otherwise an operator's rows are read out of its own index into the next.
An IN's index takes its affinity as they are read out of it, not before they are told apart ("No affinity transformations are applied to any values when comparing rows as part of a compound SELECT").

A window query's result columns are read out of a layer of their own, so an arm's column there is the expression that layer computes (`result_column_collseq_parts`, through every layer); this is
the fix forward, not on main (the oracle on ed7007d4 found it: `diff-e4e56764cf56d337`).

The test: `crates/engine/tests/compound_collations.rs`, 1,589 queries over 56 arms, and in the fix forward 597 more over 15 arms of two columns, under the four settings with SQLite 3.54.0's answers (`bench/sqlite_compound_collations.py`),
and six held to the page, three of which SQLite answers otherwise.

| on | answers that differ, of those run | the two other tests |
|---|---:|---|
| main a7d4af72's three files, the sweep as first drawn (1,591) | 588 of 6,364 | fail |
| the fix, the sweep as first drawn | 4 of 6,364, one query, SQLite's four-arm merge | pass |
| the fix, the sweep as first landed (1,589) | 0 of 6,356 | pass |
| the fix as landed (ed7007d4's collate.rs), the sweep with windowed arms (2,186) | 32 of 8,744 | pass |
| the fix forward (3055995b, not on main), the same | 0 of 8,744 | pass |

Which of several equal texts a compound keeps no page says, and Amphora keeps the last where SQLite keeps the first; the sweep folds them, as the oracle does.

## Checks

Linux, adacachy2, the tree as landed on main (ed7007d4: the fix on c34b906a, lane C26's): clippy `-p amphora-engine --all-targets --locked -D warnings` 0 ("Checking amphora-engine"); check-production 0 ("10515 left in the engine's
production code across 17 lints, as the ratchet holds", `indexing_slicing` lowered by the 8 the fix removed); `cargo fmt --all --check` 0; compound_collations 3 passed, oracle_findings 102, cte_scopes 1, in_subquery 4, refused_queries 6.
On d87c1860 (before C26's landing): the same five, and cte_columns 11, columns_read 5, row_values 4, outer_join_subqueries 3, plan_cache 7, without_rowid_dml 4, real_text 2,
window_columns 9, nested_loops 7, row_value_seek 5, records_plans 5, windows_and_triggers 4, 27 lib tests (`collat`, `compound`, `recursive`); skip_scan 3 of 4, the fourth failing on main as well.
The oracles' unit tests 11 passed, 0 failed (the module alone on the engine, release); bench clippy 0 ("Checking amphora-bench"). No bench source was changed.

## After the landing

The oracle on ed7007d4, six processes of 260 s (27021 to 27023 the differential check, 27024 to 27026 every oracle but CERT and the order check): 53,008 databases, 2,115,673 checks, 445 findings, every one replayed at the four settings: 422 RX, 7 AH, 2 RW, 3 CK, 1 CI,
1 DX, 5 errors (C11's CM 2, C11's CN 1, CW or CJ 2), no panic, no hang. The other four are wrong rows: a windowed arm's (`diff-e4e56764cf56d337`, above), ED (`same-170a0802191293b0`, wrong data, on 988f0fb4 too), and `same-11c1089ee6684474` and
`same-dddb106a96a38525`, which keep another of two equal texts (no page says; the `same` check compares them exactly, and they differ on 988f0fb4 as well). The first run's 1,581 findings answer on ed7007d4 as on the branch: 1,581 of 1,581.

## Refusals and setup

Amphora refuses what SQLite answers, the differential check, a value's error aside (31,503 of 100 messages: JSON, ESCAPE, an integer overflow; C11: 39,106 of 124).
C11's 83,940 of `ORDER BY term does not match` and 4,616 of `secondary index lacks primary key locator` are gone, and its `no such table: w#` 1,293, `table not found` and `parenthesized conditional`.

| Amphora's message | count | under |
|---|---:|---|
| `Parse error: insufficient registers allocated for expression vector write (start=#, count=#, next_free=#)` | 13,945 | row value 12329, derived 4782, window 4059, view 3201 |
| `Parse error: row value misused` | 247 | row value 226, derived 82, view 62, union 59 |
| `Parse error: incomplete infinity` | 2 | derived 2, window 2 |
| `Parse error: Expected true` | 1 | window 1, with 1 |

Statements refused as the databases are made (C11's `near "STRICT"` 7,701, `NOT NULL constraint failed` 4,045 and `triggered mutations of WITHOUT ROWID tables` 2,423 are gone):

| Amphora's message | count | such as |
|---|---:|---|
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

`incomplete infinity` is DY; `Expected true` is a value's error in a column nothing reads (CW's form).

## Reproduce

```sh
amphora-bench --oracle --oracles diff --seed 27001 --seconds 620 --reduce-seconds 3 --features bench/corpus/features.json --sqlite "$(bash bench/oracles/sqlite.sh)" --out DIR
amphora-bench --oracle --sqlite LIB --replay bench/records/2026-11-24-adacachy2-oracle-c27/findings/<file>.md     # the four settings, one at a time
python3 bench/records/2026-11-24-adacachy2-oracle-c27/selfcheck27.py LIB REPLAY.json SELF.json                    # SQLite against itself
python3 bench/records/2026-11-24-adacachy2-oracle-c27/plain29.py DIR                                              # the plain forms, for --replay
```
