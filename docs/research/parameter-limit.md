# A LIMIT that is a parameter: E5's keyset pages, and what other engines do

Source notes for `docs/design/planner.md` decision 17 (CLAUDE.md §5) and the unify audit's E5
(kiln goal c64d544897cf8adca2c153ed05e2c528): a keyset page, `(a,b,c) > (?,?,?) ORDER BY a,b,c
LIMIT n`, read by a scan of the table and a sort where sqlite3 seeks the index. Kiln's research
record is 01a198ac2ce772688e9b4e6e08cbaa5d.

## 1. The check at main 29be700

Amphora at main 29be700, which holds 327a8f9 (decision 15) and 060cc1c (decision 16): a scratch
test of the engine (EXPLAIN QUERY PLAN), studio-3, 2026-11-01 22:51Z, load 7.8. sqlite3 3.54.0
(2026-10-09) on adacachy, in memory. Both on the same DDL, rows and statements:
`t(a TEXT NOT NULL, b BLOB NOT NULL, c INTEGER NOT NULL, v BLOB)`, 4,000 rows over 7 values of a.
The index kinds are a rowid table's primary-key autoindex (`PRIMARY KEY (a, b, c)`), an explicit
index (`t_i ON t(a, b, c)`) and a WITHOUT ROWID primary key. Each was planned without statistics
and after ANALYZE. Every read but the covering ones takes `v`, which the index lacks.

Through the autoindex (the explicit index plans the same, naming `t_i`):

| Read, `ORDER BY a, b, c` | Amphora, no statistics | Amphora, after ANALYZE | sqlite3 3.54.0, both |
|---|---|---|---|
| past a row value, `LIMIT 8` | SEARCH `((a,b,c)>(?,?,?))` | SEARCH | SEARCH |
| past a row value, covering, `LIMIT 8` | SEARCH, covering | SEARCH, covering | SEARCH, covering |
| below a row value, descending, `LIMIT 8` | SEARCH `((a,b,c)<(?,?,?))` | SEARCH | SEARCH |
| past the first column, `LIMIT 8` | SEARCH `(a>?)` | SEARCH | SEARCH |
| from the start, `LIMIT 8` | SCAN USING INDEX | SCAN USING INDEX | SCAN USING INDEX |
| past a row value, `LIMIT ?4` | **SCAN t, USE SORTER** | SEARCH | SEARCH |
| past a row value, covering, `LIMIT ?4` | SEARCH, covering | SEARCH, covering | SEARCH, covering |
| past the first column, `LIMIT ?2` | **SCAN t, USE SORTER** | **SCAN t, USE SORTER** | SEARCH `(a>?)` |
| from the start, `LIMIT ?1` | **SCAN t, USE SORTER** | **SCAN t, USE SORTER** | SCAN USING INDEX |
| past a row value, no LIMIT | SCAN t, USE SORTER | SEARCH | SEARCH |
| past a row value, covering, no LIMIT | SEARCH, covering | SEARCH, covering | SEARCH, covering |
| past the first column, no LIMIT | SCAN t, USE SORTER | SCAN t, USE SORTER | SEARCH `(a>?)` |

- Under a literal LIMIT, no read sorts where sqlite3 doesn't, through any of the three index kinds.
  060cc1c's test, `rowid_index_order.rs`, failed at 327a8f9 with `SCAN rp` and `USE SORTER FOR
  ORDER BY`. That is the check of 327a8f9 the goal asks for.
- Under a parameter LIMIT, 10 reads sort where sqlite3 seeks or scans the index. Without
  statistics there are three per rowid index kind. After ANALYZE there are two per kind, since a
  row value then seeks.
- The WITHOUT ROWID key plans as sqlite3 in every form: `SEARCH t USING PRIMARY KEY (...)` or
  `SCAN t`. ANALYZE refuses that database, "ANALYZE on tables without rowid is not supported"
  (kiln goal 5adc688955008b23b0bf7490eee18a45), so its plans after ANALYZE are untested.
- Kiln's conversation feed, on kiln's own DDL (`records`, `records_by_rev ON records (rev)`, kiln
  dc19ac9e52 `crates/kiln/src/convo/sql.rs:52-61`), has the same shape. `FEED_RECORDS` reads `rev >
  ?1 ORDER BY rev LIMIT ?2` (line 189) and `RECORDS_BELOW` reads `rev < ?1 ORDER BY rev DESC LIMIT
  ?2` (line 155). Without statistics both plan as `SCAN records` and `USE SORTER FOR ORDER BY`, so
  each feed page reads and sorts every record line. After ANALYZE, and under a literal `LIMIT 500`,
  they search `records_by_rev`. sqlite3 searches `records_by_rev` for all of them. Kiln binds every
  LIMIT it writes as a parameter (`crates/kiln/src/records.rs:207`, `ORDER BY v.key LIMIT ?6`). Where
  Turso sorted, it names an index instead (`crates/kiln/src/records/turso.rs:76-80`, "Turso plans
  these by the primary key and sorts the family's every version ..., so they name the index").

## 2. Why: a row goal comes only from a literal

`crates/engine/translate/optimizer/mod.rs:1163-1175` sets decision 16's row goal from
`literal_limit_rows` alone, so `LIMIT ?` leaves an ordered read costed for its whole range.
Without statistics the model takes the table at a million rows (`rows_per_table_fallback`), 50 to a
page, with a 2,000-page cache (`cost_params.rs:105-125`). Each lookup is then a page read (decision
5). Decision 16 put the read at about 371,709 past a bound and 929,109 from the start, against
37,888 to 62,863 for a scan and a sort.

## 3. What other engines do with a LIMIT that is a parameter

SQLite 3.54.0 (github.com/sqlite/sqlite, tag `version-3.54.0`):
- The LIMIT reaches the cost estimates only as an integer literal. `src/select.c:2603-2611`,
  `computeLimitRegisters`: `if( sqlite3ExprIsInteger(pLimit->pLeft, &n, pParse, 0) ){` ...
  `p->nSelectRow = sqlite3LogEst((u64)n); p->selFlags |= SF_FixedLimit;`. Otherwise the LIMIT is
  only code. `src/select.c:8436`: `assert( WHERE_USE_LIMIT==SF_FixedLimit );`.
  `src/where.c:5577-5588`, `whereSortingCost`, takes the LIMIT for M only `if( (pWInfo->wctrlFlags
  & WHERE_USE_LIMIT)!=0 )`.
- So under `LIMIT ?` its weights pick the index. A full scan costs 3.0×N, `src/where.c:4158-4161`:
  "The 3.0 factor is an extra cost designed to discourage the use of full table scans, since index
  lookups have better worst-case performance if our stat guesses are wrong" (`pNew->rRun = rSize +
  16`, line 4173). A table lookup adds `nOut + 16`, about three rows' worth (line 3575).

PostgreSQL (github.com/postgres/postgres, `00d33e82bbb087d0f77fb5f49475885db62d1e37`):
- A generic plan guesses 10% of the result. `src/backend/optimizer/plan/planner.c:2946-2954`,
  `preprocess_limit`: "However, if it's not a constant LIMIT then we have to guess; for lack of a
  better idea, assume 10% of the plan's result is wanted." Then `limit_fraction = 0.10;`.
- A prepared statement's first executions are planned with their values.
  `src/backend/utils/cache/plancache.c:1211-1213`, `choose_custom_plan`: "Generate custom plans
  until we have done at least 5 (arbitrary)".

Neither fits Amphora as it is:
- Plans are fixed at prepare (`bench/planner/audit/08-fixed-at-prepare.sql:1-2`: "new statistics
  and a parameter's value never re-plan it"), and EXPLAIN QUERY PLAN of an unbound statement is the
  plan kiln runs. So a plan made from bound values would not be sqlite3's plan as EXPLAIN shows it.
- PostgreSQL's 10% reaches some reads but not others in Amphora's model. From the start it is
  100,000 lookups of the million fallback rows, about 92,000 against 62,863, so it stays a scan and
  a sort. Past a bound it wins by about 2% (about 37,000 against 37,888).
- SQLite's weights are a model of their own: decision 5 measured a lookup at a page read where
  the cache can't hold the table.

The estimate without a guessed constant: Moerkotte, Neumann and Steidl, "Preventing Bad Plans by
Bounding the Impact of Cardinality Estimation Errors", PVLDB 2(1):982-993, 2009
(doi:10.14778/1687627.1687738), section 2. It defines the q-error, the factor by which an estimate
is off either way ("We now define the q-error, which is multiplicative in nature"). Of a set of
values it takes the q-middle, the square root of the least times the greatest, and shows "the
q-middle minimizes the q-error".

The rows a read under `LIMIT ?` keeps lie between the fewest the statement can keep (1, more under
a literal OFFSET or LIMIT) and all its range keeps. The q-middle of those two is off by at most the
square root of their ratio, whatever value is bound. 10% of the range is off by up to a tenth of
the range for a page of one row.

## 4. What the no-LIMIT reads keep

A whole range read through an index that lacks a column scans and sorts here, where sqlite3
seeks. Decision 5 measured that choice on fedora: a 200,000-row table read in an index's order with
a lookup a row took 257 ms (90 ms all cached), against 75 ms (36 ms) to scan and sort it. These
reads are not E5's pages, and decision 17 leaves them as they are. The same measurement bounds the
cost of an ordered read under a LIMIT bound to the whole range, at about 3.4× the scan and sort.
For a page of eight, a scan and a sort read all 4,000 rows at 327a8f9, against at most 16 rows for
an ordered read.
