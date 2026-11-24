# Lane C8: the oracles run wide on main, their findings ranked, the worst wrong answer fixed (2026-11-24, adacachy2)

Losses first.

- **Two defects that give no answer or a wrong one are left on main, traced, not fixed (one fix was the brief).**
  - CG, a wrong answer in plain SQL at defaults: a LEFT JOIN drops left rows. Below.
  - CH, a panic in production code (RIGHT and FULL JOINs over a WHERE that holds nested IN subqueries).
- CI (json_pretty's indent) and CJ (an error SQLite does not raise) are wrong too, and rarer.
- CL: 372 CERT and order-check findings (estimates, no answer) where the runs of 2026-11-06 (8567e7b) had
  none in 540,848 checks of each. Not traced further here than the two shapes below.
- CK: 245 findings are the 15 digits a REAL's text has by decision (`numeric/fp.rs`), where SQLite 3.54.0 writes 17
  (226 by their decoded digits, 10 read by hand, 9 through length() or octet_length() by their form alone).
  They are most of what the differential check finds now, and they spend its reduction time (up to 8 s each).
- The runs cut short for room (seeds 901 to 904, 1002, 1013, 1014) left findings and no summary, so their checks are in no
  total here. Every check total below is of a run that ended.
- CPU, user and system: 5,138 s timed for seeds 1001, 1011, 1012 and 1015 on main; the other runs were not timed, and by their wall time at the 0.84 of a core the process list showed (9 min 10 s in 11 min for three of them at 08:22Z) they make it about 2.5 hours in all (inferred).
- `crates/engine/tests/planner.rs:182` fails on the merged tree as on main before it (lane C4's record: red since 18daab1e).
- `--replay`'s four settings (`bench/src/oracle/mod.rs`, this lane's tool, the ranking's) land with this record; the bench's clippy passes
  (0, "Checking amphora-bench", Linux) on 10c2bf07 and on the tree this lands on.

## What ran

- Reference: SQLite 3.54.0, `bench/oracles/sqlite.sh` (LIKE_DOESNT_MATCH_BLOBS). Also adacachy2's system SQLite 3.54.0, to tell AH.
- Build: release, the oracles' module (`bench/src/oracle`, unchanged but for `--replay`) compiled alone on the engine with the
  bench's own features (`fs, json, pure-rust-crypto, checksum, bench, io_uring`), so no competitor is linked. Engine: main
  67cc7e69 (the tree this lane began from), and lane S7's bench binary (0101aa38 by its record) for seeds 901 to 904, 911 and 912.
- `oracle-lite --oracle [--oracles diff,tlp,norec,pqs,dqp,eet] --seconds N --seed S --reduce-seconds 8 --features bench/corpus/features.json --sqlite LIB`
  one core a process, `nice -n 10`, a process a seed, adacachy2 shared (1-minute load 49 to 68 on 32 cores).

| run | engine | oracles | databases | checks | findings | more of the same |
|---|---|---|---:|---:|---:|---:|
| seed 911 | 0101aa38 | diff, dqp, eet, norec, pqs, tlp | 27,990 | 1,129,969 | 23 | 0 |
| seed 912 | 0101aa38 | diff, dqp, eet, norec, pqs, tlp | 27,822 | 1,122,595 | 28 | 0 |
| seed 1001 | main 67cc7e69 | cert, diff, dqp, eet, norec, order, pqs, tlp | 16,705 | 677,179 | 167 | 0 |
| seed 1011 | main 67cc7e69 | diff, dqp, eet, norec, pqs, tlp | 64,848 | 2,617,850 | 74 | 0 |
| seed 1012 | main 67cc7e69 | diff, dqp, eet, norec, pqs, tlp | 67,587 | 2,728,394 | 76 | 0 |
| seed 1015 | main 67cc7e69 | dqp, eet, norec, pqs, tlp | 63,924 | 2,572,452 | 2 | 0 |

268,876 databases, 10,848,439 checks. Every oracle checked cases; CERT and the order check in seed 1001 only.

| oracle | checks | passed | failed (before dedupe) | skipped | refused | other plan |
|---|---:|---:|---:|---:|---:|---:|
| diff | 1,338,470 | 1,094,766 | 202 | 241,382 | 2,120 | 0 |
| dqp | 1,849,855 | 1,416,562 | 1 | 433,292 | 0 | 488,915 |
| eet | 1,849,855 | 1,493,748 | 0 | 356,107 | 0 | 0 |
| norec | 1,849,853 | 1,704,133 | 4 | 145,716 | 0 | 0 |
| pqs | 1,674,635 | 1,550,664 | 3 | 123,968 | 0 | 0 |
| same | 268,876 | 268,876 | 0 | 0 | 0 | 0 |
| tlp | 1,849,853 | 1,592,576 | 2 | 257,275 | 0 | 0 |
| cert | 83,521 | 83,420 | 94 | 7 | 0 | 0 |
| order | 83,521 | 83,457 | 64 | 0 | 0 | 0 |

No hang. No setup statement was refused or left other rows (`same`: every database's rows matched). Panics: CH.

### Skips, by reason

| oracle | reason | count |
|---|---|---:|
| diff | a bare column: its row is any of its group's | 122,225 |
| diff | sqlite refused | 103,036 |
| diff | a LIMIT no longer keeps certain rows | 16,056 |
| diff | SQLite's answer holds ill-formed UTF-8 (garbage in, garbage out) | 65 |
| dqp | a bare column: its row is any of its group's | 167,890 |
| dqp | amphora refused the base | 138,185 |
| dqp | the index can't serve the query | 102,423 |
| dqp | a LIMIT no longer keeps certain rows | 22,277 |
| dqp | a derived query meets a value's error (evaluation order) | 2,515 |
| dqp | the relation fails on SQLite too | 2 |
| eet | a bare column: its row is any of its group's | 194,611 |
| eet | amphora refused the base | 135,846 |
| eet | a LIMIT no longer keeps certain rows | 22,175 |
| eet | a derived query meets a value's error (evaluation order) | 2,058 |
| eet | a derived query errors on SQLite too | 1,416 |
| eet | the relation fails on SQLite too | 1 |
| norec | amphora refused both | 127,230 |
| norec | a derived query meets a value's error (evaluation order) | 18,486 |
| pqs | sqlite can't evaluate it | 93,814 |
| pqs | sqlite misses its own pivot | 30,115 |
| pqs | a derived query meets a value's error (evaluation order) | 39 |
| tlp | amphora refused the base | 184,189 |
| tlp | a derived query meets a value's error (evaluation order) | 56,170 |
| tlp | a REAL sum's partitions round apart from it | 16,848 |
| tlp | a partition's sum overflows | 66 |
| tlp | the relation fails on SQLite too | 2 |
| cert | sqlite refused | 7 |

### Refusals: Amphora refuses what SQLite answers (differential check)

Most are a JSON or ESCAPE error Amphora raises for a call SQLite never evaluates (a constant or loop-invariant call computed
ahead of the rows: the findings list's cause Q, and CJ is one such met by PQS): counted, not findings, by the oracle's rule.

| oracle | Amphora's message | count |
|---|---|---:|
| diff | `Parse error: malformed JSON` | 598 |
| diff | `Parse error: Unexpected character` | 169 |
| diff | `json_object() requires an even number of arguments` | 150 |
| diff | `JSON cannot hold BLOB values` | 139 |
| diff | `json_replace() needs an odd number of arguments` | 89 |
| diff | `json_set() needs an odd number of arguments` | 88 |
| diff | `json_insert() needs an odd number of arguments` | 84 |
| diff | `JSON path error near: "#"` | 70 |
| diff | `json_array_insert() needs an odd number of arguments` | 70 |
| diff | `json_object() labels must be TEXT` | 63 |
| diff | `Parse error: Unexpected end of input` | 60 |
| diff | `JSON path error near: "#.#"` | 57 |
| diff | `FLAGS parameter to json_valid() must be between # and ##` | 48 |
| diff | `string or blob too big` | 47 |
| diff | `ESCAPE expression must be a single character` | 35 |
| diff | `Parse error: bad JSON path: ''` | 34 |
| diff | `JSON path error near: "###"` | 19 |
| diff | `Parse error: bad JSON path: '#.#'` | 15 |
| diff | `JSON path error near: "-#"` | 14 |
| diff | `Parse error: bad JSON path: '#'` | 13 |
| diff | `Parse error: bad JSON path: 'A'` | 12 |
| diff | `JSON path error near: "-###################"` | 11 |
| diff | `Parse error: bad JSON path: '%'` | 11 |
| diff | `Parse error: bad JSON path: '-#'` | 11 |
| diff | `integer overflow` | 11 |
| diff | `JSON path error near: "###################"` | 11 |
| diff | `Parse error: bad JSON path: 'true'` | 11 |
| diff | `JSON path error near: "################.#"` | 10 |
| diff | `JSON path error near: "-#.#"` | 10 |
| diff | `Parse error: bad JSON path: 'a '` | 10 |
| diff | `Parse error: bad JSON path: 'é'` | 10 |
| diff | `Parse error: bad JSON path: '_'` | 9 |
| diff | `Parse error: bad JSON path: 'abc'` | 9 |
| diff | `Parse error: bad JSON path: 'x'` | 9 |
| diff | `JSON path error near: "-#.#e-###"` | 8 |
| diff | `JSON path error near: "################"` | 7 |
| diff | `JSON path error near: "-###"` | 7 |
| diff | `Parse error: bad JSON path: '##'` | 7 |
| diff | `JSON path error near: "##"` | 6 |
| diff | `JSON path error near: "#.#e+###"` | 6 |
| diff | `Parse error: bad JSON path: '#e#'` | 6 |
| diff | `Parse error: bad JSON path: '#x##'` | 6 |
| diff | `Parse error: bad JSON path: 'ÿ'` | 6 |
| diff | `Parse error: bad JSON path: ''` | 6 |
| diff | `Parse error: bad JSON path: ' '` | 6 |
| diff | `JSON path error near: "-###.#"` | 5 |
| diff | `Parse error: bad JSON path: 'ABC'` | 4 |
| diff | `Parse error: bad JSON path: 'a'` | 4 |
| diff | `JSON path error near: "####"` | 3 |
| diff | `Parse error: bad JSON path: '###'` | 3 |
| diff | `Parse error: bad JSON path: '################'` | 2 |
| diff | `Parse error: bad JSON path: '#.#e+###'` | 2 |
| diff | `Parse error: bad JSON path: '################.#'` | 2 |
| diff | `Parse error: bad JSON path: '-###'` | 2 |
| diff | `JSON path error near: "#.################e+##"` | 2 |
| diff | `Parse error: bad JSON path: ' a'` | 2 |
| diff | `JSON path error near: "-##"` | 1 |
| diff | `Parse error: bad JSON path: '###################'` | 1 |
| diff | `Parse error: bad JSON path: '-#.#e-###'` | 1 |
| diff | `Parse error: bad JSON path: '{"a":#,"b":[#,#]}'` | 1 |
| diff | `JSON path error near: "###.#"` | 1 |
| diff | `JSON path error near: "#.###################"` | 1 |
| diff | `Parse error: bad JSON path: '##:##:##'` | 1 |
| diff | `Parse error: bad JSON path: '-###################'` | 1 |
| diff | `JSON path error near: "####.#"` | 1 |
| diff | `JSON path error near: "##.#"` | 1 |
| diff | `JSON path error near: "-#.################e+##"` | 1 |

## The ranking

Each finding (deduped by its reduced form, its file in `2026-11-24-adacachy2-oracle-c8/findings/`; `findings.tsv` there lists
all) was replayed on main's build (67cc7e69's engine) under four settings: default, parallel mode, statistics, both. "Both
answers" are SQLite's and Amphora's.

| rank | cause | findings | which oracles | settings that are wrong | SQLite / Amphora |
|---:|---|---:|---|---|---|
| 1 | CF, NOT EXISTS / NOT IN over an OR: **fixed** (fcbdf0c2) | 6 | PQS 3, differential 1, DQP 1, NoREC 1 | all four | 8 rows / 0, 6 / 0, 1 / 0; DQP: the plain form 0 rows, the hinted one 1; NoREC: kept 0, TRUE for 1 |
| 2 | CG, a LEFT JOIN drops left rows | 3 | TLP 2, differential 1 | default and parallel; not with statistics | 1 row / 0 (each) |
| 3 | CI, json_pretty's indent from a REAL | 1 | differential | all four | `100.0"a": 1` / `100"a": 1` |
| - | CH, panic | 4 | NoREC 3, differential 1 | default; with statistics 3 of the 4 (a panic ends the replay, so the parallel mode was not run) | an answer / panic |
| - | CJ, an error | 1 | PQS | all four | the pivot's row / "json_object() requires an even number of arguments" |
| by design | CK, a REAL's text digits | 245 | differential | all four | `7.3890560989306504` / `7.38905609893065` |
| not Amphora's | AH, the reference SQLite's LIKE optimization | 13 | differential | (the system's SQLite agrees with Amphora) | 0 rows / 1 row |
| estimates | CL, CERT and the order check | 372 | CERT, order | (no answer) | outer join 360,000,000,000 < its inner join 379,473,319,221; 0.5 / 0.3535 |

## CF, the worst wrong answer, traced and fixed

In the language a user writes, at defaults with no statistics, on main 67cc7e69:

```sql
CREATE TABLE o(id INTEGER PRIMARY KEY, k INT, st TEXT, v INT);
CREATE TABLE r(id INTEGER PRIMARY KEY, k INT, kind INT, v INT);
INSERT INTO o VALUES (1,1,'open',1),(2,2,'closed',2),(3,3,'held',3),(4,1,'open',9),(5,2,'held',NULL);
INSERT INTO r VALUES (1,1,1,5),(2,2,2,6),(3,3,1,7);
SELECT id FROM o WHERE NOT EXISTS (SELECT 1 FROM r WHERE r.k = o.k AND
  ((o.st = 'open' AND r.kind = 1) OR (o.st = 'closed' AND r.kind = 2)));
-- SQLite: 3, 5.   Amphora (main 67cc7e69): no row.
```

Seven of eleven such statements (an equality with another literal in each branch, the same term in all, a join key beside the
OR, two inner tables, under a LEFT JOIN, NOT IN) answered wrongly at defaults, and an EXISTS over the same condition rightly.

- **Plan:** `SCAN o`, `SCAN unnested_subquery_t2` (the anti join: `rewrite_through_derived_table`, optimizer/unnest.rs), `SCAN r`.
- **First wrong place:** the bytecode tests `o` alone before the anti join's inner scan and sends a row that fails to the
  outer `Next`, so it is never emitted. That test is a WHERE term `rewrite_or_terms`
  (optimizer/lift_common_subexpressions.rs) took out of the subquery's OR: `o.st IN ('open', 'closed')` here, the
  table's share of the OR (`v < 2 OR v > 8`), or a term all branches hold (`o.st = 'open'`). The unnested subquery's condition
  is left in the WHERE clause as the anti join's match condition; a term on the outer table alone is then a filter, and it
  drops the rows the anti join must keep. The unnesting already refuses a whole conjunct with no inner column for that
  reason; the OR pass, run after it, took parts of one.
- **Rows, counted in the real run** (`Connection::set_row_counters`, the statement above; the engine's optimizer as on main, then fixed):

  | operator | main's optimizer: actual rows | fixed: actual rows |
  |---|---:|---:|
  | `o`, the outer scan, past its filters | 3 (of 5: both 'held' rows gone; bytecode `Eq r6 'open'`, `Ne r6 'closed'` to `Next`) | 5 |
  | `unnested_subquery_t2`, the anti join's rows out | 0 | 2 |
  | `r` read | 4 in 3 restarts of the coroutine | 3 in one pass, kept in an index on k |
  | the statement's rows | none | 3, 5 |

- **Fix** (fcbdf0c2, on main; test ec5a9ce7): `rewrite_or_terms` is given the plan's anti-joined tables. A term taken from an OR that reads one must read it
  too, which narrows the rows looked among, never the outer rows. An EXISTS (a semi join) keeps its filters.
- **Red and green:** `a_not_exists_keeps_the_rows_no_branch_of_its_or_names` (tests/oracle_findings.rs): red without the fix on
  Linux (`left: []`, `right: ["3", "5"]`, 69fad660's tree), green with it, under the four settings.
- **The databases that found it, rerun whole** (`--seed S --state N`, 41 checks each): seed 911 state 4501, 912 13268, 1011 30860, 1011 33195,
  1015 7415: 1 failed check on main's build in each, 0 on the fixed build. Every finding of the cause, replayed: `differs` on
  main, `same` fixed, at all settings; the findings of every other cause are unchanged by it.

Checks (Linux, adacachy2; the merged trees 8c42973d and 10c2bf07, whose engine is the landed tree's): clippy
`-p amphora-engine --all-targets` 0 ("Checking amphora-engine"); check-production 0 on both (10,617, as the ratchet holds);
fmt clean; oracle_findings 84 of 84, in_subquery 4, hash_semi_join 1, join_methods 4, unnesting 2 (simulator feature), the pass's
own unit tests 12, the whole `optimizer::` set 184 passed, 0 failed, 4 ignored (8c42973d, with lane C6's sweep);
planner 13 passed, 1 failed (182, main's own).

## What is left, each with its query and both answers (not fixed)

- **CG (rank 2; plain SQL, default).** `SELECT a.id, b.id FROM a LEFT JOIN b ON a.code = '255' AND b.id = a.id WHERE a.total > 100 AND a.note IS NULL`
  over `a(id INTEGER PRIMARY KEY, total REAL, code TEXT UNIQUE, note TEXT)` with an index on `total`, rows (1, 255, '255'), (2, 50, '050'), (3, 300, '099'):
  SQLite `1, 1` and `3, NULL`; Amphora `1, 1` alone. The oracles' own: `SELECT 0.0 FROM t0 AS a0 LEFT JOIN t0 AS a1 ON (a0.c2 <= 100) WHERE ((100 < a0.c0) AND ((NULL = a0.c1) ISNULL))`
  (tlp-0026d1343cdbadae: SQLite 1 row, Amphora 0) and, with a RIGHT JOIN, diff-3fcb32b9317ecd43 (1 / 0). Plan:
  `MULTI-INDEX AND a0 (sqlite_autoindex_t0_1, ...)`. Bytecode of the second: the AND's first branch runs `SeekGT` and `IdxGE` on the left
  table's index by the ON term's own bound (`1e15`), so the LEFT JOIN's ON term filters the left table. Code:
  `multi_index_can_consume_term` (optimizer/multi_index.rs) allows any term for a table no outer join null-extends, so the
  ON term of another table's LEFT JOIN is taken. Not with statistics (another plan). Seen in 3 findings (the third: tlp-6c50fd752555f572) and in plain SQL.
- **CH (panic).** `SELECT NULL FROM t1 AS a0 FULL JOIN t1 AS a1 ON NULL WHERE (SELECT count(s21.c0) FROM t0 AS s21 WHERE ((s21.c0 - s21.c0) IN (SELECT s361.c2 FROM t2 AS s361 WHERE (a1.c0 <= s361.c0))))`:
  SQLite answers; Amphora: "indexbtree_seek_unpacked: index_info required" (storage/btree.rs:3155), and in 2 of the 4 the
  statement's drop then panics too ("dirty pages should be empty for read txn", storage/pager.rs:7106). Bytecode: the unmatched-row pass's copy of the subquery holds the IN subquery
  the main pass unnested, and opens its list on cursor 0 (`OpenEphemeral 0`, the outer table's own cursor, opened as a table), then
  seeks it as an index (`NotFound 0`). My plain forms (one RIGHT and two FULL JOINs over three small tables) all matched SQLite.
- **CI.** `SELECT json_pretty('{"a":1,"b":[2,3]}', a0.c0) FROM t0 AS a0` (`c0 REAL`, 100): SQLite indents by `100.0`, Amphora by `100`.
- **CJ.** `SELECT a0.rowid FROM t1 AS a0 WHERE ((SELECT min(s235.c0) FROM t0 AS s235 WHERE (jsonb_object(7, a0.c3, 255) BETWEEN (s235.c1 << '_') AND a0.c0)) ISNULL)` (t0 empty):
  SQLite answers; Amphora: "json_object() requires an even number of arguments".
- **CL.** CERT: `SELECT '_' FROM t0 AS a0 LEFT JOIN t0 AS a1 ON ('a' IS NOT a0.c0) WHERE ('1.5' < a0.c0)` estimates 360,000,000,000 rows, its inner join 379,473,319,221
  (inferred from the numbers, not traced: a table's two filters multiplied in the outer form, combined by decision 21's backoff in the inner one). Order: `SELECT 7 FROM t0 AS a0 JOIN t0 AS a1 ON (a1.c0 OR ((NOT a1.c0) AND NULL))`,
  0.5 rows, 0.3535 with the condition in WHERE.

## The oracle's gaps

It generates no window function, WITH or recursive CTE, compound SELECT (but in CERT), view, trigger, generated column,
STRICT or WITHOUT ROWID table, UPSERT, RETURNING, row value, USING or NATURAL join, derived table in FROM, FILTER, LIMIT in a
subquery or `json_each`; its tables hold at most a few dozen rows, and it compares rows as multisets, so a lost ORDER BY
(lane C1's defect) is seen only where a LIMIT cuts the result.

## Reproduce

```sh
bash bench/oracles/sqlite.sh                 # the reference
amphora-bench --oracle --seed 1011 --seconds 1500 --reduce-seconds 8 --features bench/corpus/features.json --sqlite LIB --out DIR
amphora-bench --oracle --sqlite LIB --replay bench/records/2026-11-24-adacachy2-oracle-c8/findings/<id>.md   # the four settings
```
