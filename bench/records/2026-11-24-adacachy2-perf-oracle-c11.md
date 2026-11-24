# Lane C11: the oracle makes the features it never made, a REAL's digits are no finding, and a run on main (2026-11-24, adacachy2)

Lane's hour: 08:47:17Z to 09:47:17Z. Losses first.

- **Nothing is fixed.** None of these is a one-line fix I would name. Nine wrong-answer causes and three panics are left on main; eight of the
  nine and all three panics are in plain SQL below, every one at default settings.
- **The reducer's slowest class drowned the first campaign.** Row values failed so often (CM, CT) that a first campaign of five
  processes (seeds 6001 to 6005, tree f47d8ae5) was stopped or left, and row values were made seldom (0.015 a predicate) for the
  run below. Its counts are in no total here.
- **Causes are named by rules, not read one by one.** The 862 deduped findings were sorted by rules over their reduced forms (`rank.py`, `look.py`
  beside this). The 11 no rule named were read by hand, and the smallest of each cause; the rest of a cause's findings were not. CT's 323 and CU's 183 may
  hide other causes behind theirs.
- **The REAL digits leave a residue.** The campaign's binary (4913dfdb) reported 80 digits findings of 862 (9.3%; lane C8's were
  245 of 633, 38.7%). The landed code answers 65 of them as the same (each replayed on it). 15 are still reported: JSONB's bytes (9: SQLite's JSON
  writes `%!0.17g` whatever FP_DIGITS is), a length over rows of which only some REALs' 15 digits convert back (3), a subnormal against other digits (1), a REAL
  compared as TEXT (2).
- `SQLite refused` skips 448,209 of the differential check's 4,030,954 and `a bare column` 454,498: a quarter of it is not compared. Not worked on.
- INSTEAD OF triggers (12,547 statements), `STRICT` beside another table option (7,701) and triggers on WITHOUT ROWID tables (2,423) are
  refused at setup, so those databases hold less than their seeds say.
- One false finding: an UPSERT that sets an INTEGER PRIMARY KEY to 9223372036854775807 makes the next rowid random (same-da6c4556632ea17b).

## What ran

- Reference: SQLite 3.54.0, `bench/oracles/sqlite.sh`; also adacachy2's system SQLite 3.54.0, to tell AH.
- Engine: main f183dd75 (the tree this lane began from). The oracles' module alone on the engine (`oracle-lite`, lane C8's), release,
  the bench's own features with `io_uring`, the database opened as production opens it (WITHOUT ROWID on).
- `oracle-lite --oracle --oracles L --seconds 760 --seed S --reduce-seconds 3 --features bench/corpus/features.json --sqlite LIB`: six
  processes, one core each (`nice -n 10`, three runs of `needs cpus=2`), 40 checks a database, one to three tables of one to four columns
  and up to 33 rows each, under `timeout 900`. adacachy2 shared, 1-minute load 38 to 69 on 32 cores. Nothing was timed.

| run | oracles | databases | checks | findings | more of the same | seconds |
|---|---|---:|---:|---:|---:|---:|
| main-8001 | diff | 28,674 | 1,171,028 | 292 | 0 | 760 |
| main-8002 | diff | 28,756 | 1,174,648 | 266 | 0 | 760 |
| main-8003 | diff, dqp, eet, norec, pqs, tlp | 27,114 | 1,055,756 | 178 | 1,733 | 760 |
| main-8004 | diff, dqp, eet, norec, pqs, tlp | 27,135 | 1,057,942 | 171 | 1,708 | 760 |
| main-8005 | diff | 30,007 | 1,225,206 | 314 | 0 | 760 |
| main-8006 | diff, dqp, eet, norec, pqs, tlp | 28,174 | 1,098,100 | 190 | 1,754 | 760 |

**169,860 databases, 6,782,680 checks.** Every run ended by its own clock; no hang, 1 check over a second.

| oracle | checks | passed | of them, digits alone | failed (before dedupe) | skipped | refused | other plan |
|---|---:|---:|---:|---:|---:|---:|---:|
| diff | 4,030,954 | 2,983,743 | 237 | 640 | 903,169 | 143,402 | 0 |
| dqp | 526,461 | 373,853 | 0 | 19 | 152,589 | 0 | 109,193 |
| eet | 547,278 | 391,700 | 0 | 1,248 | 154,330 | 0 | 0 |
| norec | 547,263 | 484,253 | 0 | 2,826 | 60,184 | 0 | 0 |
| pqs | 414,159 | 382,881 | 0 | 598 | 30,680 | 0 | 0 |
| same | 169,332 | 168,412 | 66 | 101 | 491 | 328 | 0 |
| tlp | 547,233 | 464,346 | 0 | 647 | 82,240 | 0 | 0 |

`digits alone`: the answers agree only once each REAL's text is in floatingpoint.html 1.3.1's digits (digits.rs); counted as passed, 303 in all.

### Skips, by reason

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

### Findings per feature

A check counts under each feature its queries show (a `same` check under each its database's statements hold, beside `databases`); a finding
under each its reduced form shows. Counts of findings include the digits, the reference's own and the generator's (CK, AH, RW below), and 527
panics of one statement (CP) that each database had its own name for (fixed since: one finding).

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

## The ranking

862 findings once CP's 527 are one. Each was replayed on `b1c8e304` (the same engine) under default, parallel mode, statistics and both. Every Wrong
finding differs at all four settings but those marked; a panic ends the replay.

| rank | cause | findings | which oracles | what differs | plain SQL, SQLite / Amphora |
|---:|---|---:|---|---|---|
| 1 | CT, a row value IN (subquery) with a NULL in the row | 323 | differential 237, PQS 70, NoREC 4, TLP 4, EET 2, same 6 | all four settings | `SELECT id FROM o WHERE (region, code) NOT IN (SELECT region, code FROM closed)`: 2, 3 / 3, 4 |
| 2 | DA, `excluded.<INTEGER PRIMARY KEY>` is NULL in a STRICT table's UPSERT (data written wrong) | 28 | same | all four | `DO UPDATE SET last = excluded.id`: 10, 6, 10 / 10, 6, NULL |
| 3 | CU, a recursive CTE's number joined to a TEXT column finds nothing | 183 | differential | all four | `SELECT 1, 0 UNION ALL SELECT r.n + 1, r.d + 1 FROM r JOIN edge ON edge.src = r.n ..`: 3 rows / 1 |
| 4 | DB, a row value BETWEEN across a join | 2 | differential | default and parallel, not statistics (plain form); all four (findings) | `ON ((e.day, f.hour) BETWEEN (1, 10) AND (2, 4))`: 4 rows / 0 |
| 5 | CY, a compound's duplicates are told by the wrong arm's collation | 20 | differential | all four | `SELECT a FROM u UNION SELECT a FROM t` (t.a NOCASE): 'ABC', 'abc', 'x' / 'ABC', 'x' |
| 6 | CX, a trigger's new.c and old.c lose the column's collation | 6 | same | all four | `WHEN new.name = 'admin'` (NOCASE), name 'ADMIN': fires / never |
| 7 | CV, a BEFORE trigger on a STRICT table sees the value before its column converts it | 54 | same | all four | `INSERT INTO price VALUES (1, 100)` (REAL): 100.0 real / 100 integer |
| 8 | CZ, an unqualified USING or NATURAL column of a FULL or RIGHT JOIN keeps the left column's affinity | 6 | differential | all four | `SELECT y FROM a FULL JOIN b USING (k) WHERE k = 2`: 2 / no row |
| - | CO, panic: lag, lead, first_value, nth_value read twice beside another window | 88 | differential 60, EET 21, DQP 7 | panic at default | `SELECT id, sum(v) OVER (ORDER BY id), lag(v + 1) OVER (ORDER BY date(at), id), coalesce(lag(v + 1) OVER (ORDER BY date(at), id), 0) FROM ev` |
| - | CP, panic: a trigger AFTER UPDATE OF on a WITHOUT ROWID table | 527 of one | setup | panic | `UPDATE t SET v = 'y' WHERE k = 'a'` |
| - | DC, panic: "cursor id # is None" | 3 | differential | panic | a correlated NOT IN in a FULL JOIN's GROUP BY |
| - | CQ, error: a FULL JOIN with a WITHOUT ROWID table | 1 (4,616 refusals) | NoREC | default and parallel; not with statistics | `SELECT count(*) FROM o FULL JOIN w`: 4 / "secondary index lacks primary key locator" |
| - | CM, error: a row value compared or BETWEEN with a call or a literal | 2 (14,536 refusals) | NoREC, PQS | all four | `SELECT ('a', 1) BETWEEN ('a', 0) AND ('a', 2)`: 1 / "insufficient registers allocated for expression vector write" |
| - | CW, error: a view's column nothing reads is evaluated | 12 | PQS | all four | `SELECT count(*) FROM v` over a view with json_extract(body): 2 / "malformed JSON" |
| - | CN, CR, CS, errors: `row value misused`; `InSelect is not supported in this position`; `Scalar subquery is not supported in this context` (a subquery in a window's argument) | 1 each | TLP, EET | all four | `SELECT a, sum((SELECT max(b) FROM s)) OVER (ORDER BY a) FROM t`: 2, 4 / error |
| 9 | DE, a column's collation is lost in an aggregate's argument beside a correlated IN in GROUP BY | 1 | differential | all four | `max(quote(c1 IS 'abc'))` (c1 NOCASE, 'ABC'), in diff-ae362c4fee7a40c3: 1 / 0 |
| by design | CK, a REAL's text digits | 80 | differential, same 1, PQS 1 | none now; 15 still | |
| not Amphora's | AH, the reference SQLite's LIKE optimization | 26 | differential 25, same 1 | (the system's SQLite agrees with Amphora) | |
| not Amphora's (inferred) | RW, SQLite's window over a GROUP BY: `dense_rank() OVER (ORDER BY c DESC)` puts NULL first, or one group comes twice | 15 | differential | all four | the system's SQLite 3.54.0 is not Amphora's either on any of the 15 (replayed); two shapes run on it by hand answer as the oracle's does |
| not Amphora's, or no document says | json_pretty's indent from a REAL or a NUL (CI) | 3 | differential | all four | |
| no document says | which of two unique constraints an UPSERT without a target meets first (same-0e667be14465b015); a COLLATE inside quote() in a compound, beside CY (2) | 3 | same | all four | |
| not compared | text that isn't well-formed UTF-8 through lower(jsonb()); a random rowid (above) | 1, 1 | | | |

By SQLite's documents (read 2026-11-24 09:34Z, but for the last):

- CT: rowvalue.html 2.1, "The overall result of comparison is NULL if it is possible to make the result either true or false by substituting alternative
  values in place of the constituent NULLs." (NULL, 8) against ('eu', 7) is false whatever the NULL is, so `IN` is 0 and Amphora's NULL is wrong; (NULL, 7) may be
  either, so it is NULL and Amphora's 0 is wrong.
- CU: datatype3.html 4.2, "If one operand has TEXT affinity and the other has no affinity, then TEXT affinity is applied to the other operand." A CTE's number has none.
- The generator gives a compound's column one affinity in every arm because datatype3.html 3.3.1 says "it is indeterminate which of the SELECT statements will be used to
  determine affinity". Amphora and SQLite do differ there (first smoke run: `WITH RECURSIVE w(c0) AS (SELECT 1 UNION ALL SELECT CAST(..))`, then `'-1' > c0`); it is no finding.
- CY: lang_select.html compares a compound's rows as the operands of `=`, the left's collation first when neither is explicit. That is from memory: the page as fetched was cut before the
  sentence. By it `SELECT a FROM u UNION SELECT a FROM t` (u.a BINARY, t.a NOCASE) keeps 'ABC' and 'abc', as SQLite does. `SELECT a FROM u EXCEPT SELECT 'q' COLLATE NOCASE` is the one
  where Amphora's answer (1 row) fits it and SQLite's (2) does not.

### Plain SQL, each on the four settings (`plain.txt`, its scripts beside it; 67 files of one to six queries, 32 with one that differs, panics or is refused)

```sql
CREATE TABLE item(id INTEGER PRIMARY KEY, qty INT, last INT) STRICT;
INSERT INTO item VALUES (10, 1, 0);
INSERT INTO item VALUES (10, 5, 0) ON CONFLICT(id) DO UPDATE SET qty = item.qty + excluded.qty, last = excluded.id;
-- SQLite 10, 6, 10   Amphora 10, 6, NULL.  (Not STRICT: the same as SQLite.)  SET id = excluded.id fails: "datatype mismatch".
CREATE TABLE o(id INTEGER PRIMARY KEY, region TEXT, code INT); CREATE TABLE closed(region TEXT, code INT);
INSERT INTO o VALUES (1, 'eu', 7), (2, NULL, 8), (3, 'us', 9), (4, NULL, 7); INSERT INTO closed VALUES ('eu', 7);
SELECT id, (region, code) IN (SELECT region, code FROM closed) FROM o;  -- 1, 0, 0, NULL   Amphora 1, NULL, 0, 0
CREATE TABLE edge(src TEXT, dst TEXT); INSERT INTO edge VALUES ('1', '2'), ('2', '3');
WITH RECURSIVE r(n, d) AS (SELECT 1, 0 UNION ALL SELECT r.n + 1, r.d + 1 FROM r JOIN edge ON edge.src = r.n WHERE r.d < 5) SELECT n, d FROM r;
                                                                    -- 3 rows   Amphora 1
CREATE TABLE t(k TEXT PRIMARY KEY, v TEXT) WITHOUT ROWID; CREATE TABLE lg(a);
CREATE TRIGGER g AFTER UPDATE OF v ON t BEGIN INSERT INTO lg VALUES (old.k); END;
INSERT INTO t VALUES ('a', 'x'); UPDATE t SET v = 'b' WHERE k = 'a';  -- Amphora panics (translate/trigger_exec.rs:708); without OF, refused
```

Where each panics: window.rs:3457 (CO), trigger_exec.rs:708 (CP), vdbe/mod.rs:1607 and vdbe/execute.rs:7390 (DC). The forms that answer as SQLite does are in
`plain.txt` too: an UPSERT on an ordinary table, a window in ORDER BY alone, INSERT OR IGNORE with NOT NULL and `row value misused`'s plain forms among them.

## Refusals and setup

Amphora refuses what SQLite answers, the differential check, a value's error aside (39,106 of 124 messages: JSON, ESCAPE, an integer overflow):

| Amphora's message | count | under |
|---|---:|---|
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

The first is `ORDER BY (1 COLLATE NOCASE)` (parenthesized) over a compound; `ORDER BY 1 COLLATE NOCASE` is answered. Statements refused as the databases are made:

| Amphora's message | count | such as |
|---|---:|---|
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

`INSERT OR IGNORE` of a NULL into a STRICT table's TEXT PRIMARY KEY ends with "NOT NULL constraint failed" where SQLite ignores the row (plain form; WITHOUT ROWID tables do as SQLite does).

## The generator and the digits

- The features (synth.rs): compounds, WITH and WITH RECURSIVE (a counter, a UNION ended by its own duplicates, a walk along a table to a depth), derived tables,
  window functions (every inventory one, six aggregates, ROWS, RANGE and GROUPS frames, EXCLUDE, FILTER), USING and NATURAL, row values, UPSERT, views, triggers
  (BEFORE, AFTER, UPDATE OF, INSTEAD OF, RAISE), STRICT and WITHOUT ROWID, `SELECT *`. CERT and the order check stay on the old shapes. Tests: each feature is made, by name and by its text.
- What is certain is checked (`Sel::certain`), so a cut that isn't is no case: docs/research/oracles.md lists each rule.
- The digits: docs/research/oracles.md. Four tests hold the documented difference and that any other stays a finding.

## Checks

Linux, adacachy2, the code as landed on main 97cd0535 (lane C10's CH fix in it) and again on main af8ca01c (lane C9's CG fix): bench clippy 0 ("Checking amphora-bench"); the oracles' unit tests 11 passed, 0 failed, run as the module alone on the engine (the bench's
own binary needs DuckDB; not built). rustfmt clean on `bench/src/oracle`. No engine code changed.
Every plain form was replayed again on that tree: the same 38 queries differ and the same 8 files panic, so each cause above is on main 97cd0535. CG's and CH's fixes are not in the campaign's engine (f183dd75).

## Reproduce

```sh
amphora-bench --oracle --oracles diff --seed 8001 --seconds 760 --reduce-seconds 3 --features bench/corpus/features.json --sqlite "$(bash bench/oracles/sqlite.sh)" --out DIR
amphora-bench --oracle --sqlite LIB --replay bench/records/2026-11-24-adacachy2-oracle-c11/findings/<file>.md     # the four settings, one at a time
```
