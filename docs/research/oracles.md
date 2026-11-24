# Test oracles for logic bugs and slow plans

Sources for G2 step 3 (kiln plan "G2 The competition corpus"): the six oracles the step names, each
from its paper (the PDFs and their text are kept under `~/.cache/research-src/corpus-papers/`), with
SQLancer's implementation (github.com/sqlancer/sqlancer at 1d797ee, 2026-10-21) where it has one,
and what each takes to run on Amphora.

## The oracles

**TLP, Ternary Logic Partitioning** (Rigger and Su, "Finding Bugs in Database Systems via Query
Partitioning", OOPSLA 2020). A query Q and any predicate p: Q's rows are, as a multiset, the union of
Q WHERE p, Q WHERE NOT p and Q WHERE p IS NULL, since a predicate is TRUE, FALSE or NULL for each row.
The same partitioning checks GROUP BY, HAVING, DISTINCT (UNION instead of UNION ALL) and aggregates
(MIN of the partitions' MINs, SUM of their SUMs, COUNT as a SUM). 175 bugs in MySQL, TiDB, SQLite,
CockroachDB and others, 125 fixed. SQLancer: `common/oracle/TLPWhereOracle.java`,
`sqlite3/oracle/tlp/` (aggregate, distinct, group by, having).

**NoREC, Non-optimizing Reference Engine Construction** (Rigger and Su, ESEC/FSE 2020). The
optimized query `SELECT count(*) FROM t WHERE p` against a form the optimizer can't use:
`SELECT sum(count) FROM (SELECT (p) IS TRUE AS count FROM t)`, which evaluates p on every row. The
counts must agree. 159 previously unknown bugs (SQLite, MariaDB, PostgreSQL, CockroachDB), 141
fixed. SQLancer: `common/oracle/NoRECOracle.java`.

**PQS, Pivoted Query Synthesis** (Rigger and Su, OSDI 2020). Pick a row (the pivot), generate a
predicate, rectify it to TRUE on the pivot (p, NOT p or p IS NULL, by evaluating it), and query with
it: the pivot must be among the rows. Needs an evaluator of the language's semantics; SQLancer
writes its own (`sqlite3/SQLite3ExpectedValueVisitor.java`). 121 unique bugs (SQLite, MySQL,
PostgreSQL). For Amphora, SQLite is the evaluator: the predicate is evaluated on the pivot's values
by SQLite itself, so the oracle tests Amphora against SQLite's semantics, not against a copy of them.

**DQP, Differential Query Plans** (Ba and Rigger, "Keep It Simple: Testing Databases via
Differential Query Plans", SIGMOD 2024). The same query under different plans must give the same
rows; the plans come from what the engine already offers (query hints, system variables). It
reproduced 14 of the 15 bugs of TQS and found 26 new ones in MySQL, MariaDB and TiDB; 81% of its
logic bugs were out of NoREC's and TLP's reach, and it missed 86% of theirs, so the oracles add up
rather than overlap. SQLancer: `mysql/oracle/MySQLDQPOracle.java`, `tidb/oracle/TiDBDQPOracle.java`.
Amphora's plan levers: `INDEXED BY` and `NOT INDEXED` per table, `CROSS JOIN` (which fixes the join
order), secondary indexes made or dropped, statistics from `ANALYZE` or none, and the connection's
`SubqueryUnnestingMode` (`Auto`, `Disabled`, `Forced`, crates/engine/connection.rs).

**EET, Equivalent Expression Transformation** (Jiang and Su, OSDI 2024). Rewrite expressions inside
a query into equivalent ones; the results must not change. Its rules (Table 2): a boolean p becomes
`false_expr(q) OR p` or `true_expr(q) AND p`, with true_expr(q) = `q OR NOT q OR q IS NULL` and
false_expr(q) = `q AND NOT q AND q IS NOT NULL` for a random q; any expression e becomes
`CASE WHEN false_expr(q) THEN r ELSE e END`, `CASE WHEN true_expr(q) THEN e ELSE r END`, or
`CASE WHEN q THEN e ELSE e END` (r random, of e's type). 66 unique bugs, 35 of them logic bugs, in
MySQL, PostgreSQL, SQLite, ClickHouse and TiDB. SQLancer: `common/oracle/EETOracle.java`,
`EETTransformer.java`. Two of SQLite's rules narrow where the rules hold. A CASE expression has no
affinity and no collation (datatype3.html 3.2 and 7.1), so wrapping a column reference changes a
comparison it takes part in (`c = '1'` on an INTEGER column is true for 1; the CASE form compares
an integer with text and is false): the CASE rules apply only to an expression that has neither.
And the boolean rules keep a value's truth, not the value (`FALSE OR 5` is 1): they apply only where
truth is all that's read (WHERE, HAVING, ON, a CASE condition).

**CERT, Cardinality Estimation Restriction Testing** (Ba and Rigger, ICSE 2024). Cardinality
restriction monotonicity: for a query Q and a more restrictive Q′ derived from it (a conjunct added
to WHERE, an outer join made inner, a LIMIT lowered), the optimizer's estimate for Q′ must not exceed
its estimate for Q. A violation is an estimation error, which is how slow plans are chosen. 13 unique
issues in MySQL, TiDB and CockroachDB. SQLancer: `common/oracle/CERTOracle.java` (reads EXPLAIN's
estimated rows). Amphora's estimates: `EXPLAIN QUERY PLAN FORMAT=JSON` gives each table access's
`input_rows`, `rows_per_input` and `output_rows` (crates/engine/translate/eqp.rs), and at its top
the statement's own `output_rows` for every shape: a join after grouping, aggregation and a literal
LIMIT, VALUES, a SELECT of no table, a compound's arms combined (docs/design/planner.md, decision
13). CERT checks every shape the generator makes, grouped, aggregated, DISTINCT, ordered and
limited queries and compound selects (restricting an arm no EXCEPT has on its right), with no
HAVING, where fewer rows in a group can pass more groups. A query Amphora can't plan has no
estimate to check: one SQLite refuses too is skipped as such, one only Amphora refuses is counted as
a refusal, and "no estimate" is a query planned without one.

## On Amphora

Each oracle runs in `amphora-bench --oracle` (bench/src/oracle/), in one process: Amphora through its
engine crate and SQLite (rusqlite) beside it, both given the same random database. Generation draws
on the inventory (bench/corpus/features.json): its operators, its functions with the arities SQLite's
own `pragma_function_list` gives, its collations and type affinities. Every query generated is also
compared with SQLite's answer, which is the reference the step asks for; TLP, NoREC, DQP and EET hold
within Amphora alone, PQS through SQLite's evaluation, and CERT on Amphora's estimates.

What it generates beyond single SELECTs over tables (lane C11): compounds (UNION, UNION ALL, INTERSECT,
EXCEPT), WITH and WITH RECURSIVE, derived tables, window functions, USING and NATURAL joins, row values,
UPSERT, views, triggers, STRICT and WITHOUT ROWID tables. Each is made so its answer is certain, and a cut
that would leave it uncertain is no case (`Sel::certain`): a window function that tells tied rows apart has
an ORDER BY that leaves none (every table's rowid); a frame that slides sums small integers; a source
another query reads that keeps one of several equal values (DISTINCT, GROUP BY, UNION) has columns whose
equal values are the same value; a compound's column has one affinity in every arm, since which arm's it
takes is indeterminate (datatype3.html 3.3); a trigger whose rows may come in any order only writes to a
table compared by its rows alone. A query past 4,000,000 of SQLite's instructions is left before Amphora
begins it. `summary.json` counts checks, passes, failures, skips and refusals by feature, and keeps one
example of each refusal, cut down (`refusals/`; a statement's, `setup_examples`).

A REAL's text has 15 digits when they convert back, else 17 (floatingpoint.html 1.3.1), and Amphora writes
it so; SQLite 3.54.0 writes 17 for some doubles whose 15 convert back (`crates/engine/numeric/fp.rs`;
bench/records/2026-11-23-adacachy2-perf-numeric-g10.md). The differential check puts both sides' texts in
the page's digits before it compares them (`bench/src/oracle/digits.rs`), alone, inside another text and
against other digits. Where a REAL's text is inside a function's value (a length, hex, JSONB), the value may
be the one SQLite gives with its texts at 15 digits (SQLITE_DBCONFIG_FP_DIGITS). Any other difference stays
a finding: 16 digits, another double's, 15 that don't convert back, other notation. `summary.json` counts
the passes that needed it (`digits`).
