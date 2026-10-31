-- Audit row 8: a statement is planned at prepare and again only on a schema change
-- (statement.rs reprepare on SchemaUpdated): new statistics and a parameter's value never re-plan it.
CREATE TABLE t(id INTEGER PRIMARY KEY, k INT, v INT);
CREATE TABLE u(id INTEGER PRIMARY KEY, k INT);
CREATE INDEX t_k ON t(k);
INSERT INTO t(k, v) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000) SELECT i % 5, i FROM n;
INSERT INTO u(k) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 10) SELECT i FROM n;
-- Prepared before ANALYZE.
-- keep before
EXPLAIN QUERY PLAN FORMAT=JSON SELECT * FROM u JOIN t ON t.k = u.k;
ANALYZE;
-- The statement prepared before ANALYZE, run again: its plan and estimates as they were.
-- rerun before
-- The same text prepared now.
EXPLAIN QUERY PLAN FORMAT=JSON SELECT * FROM u JOIN t ON t.k = u.k;
-- A parameter: one plan for every value, its estimate the average's (k = 1 keeps 1,000 rows; k = 9 none).
EXPLAIN QUERY PLAN FORMAT=JSON SELECT * FROM t WHERE k = ?;
-- counted
SELECT * FROM t WHERE k = 1;
-- counted
SELECT * FROM t WHERE k = 9;
