-- Audit row 4: statistics are sqlite_stat1's alone (stats.rs IndexStat, TableStat): a table's rows
-- and, per index, the rows per distinct value of each leftmost prefix. Without ANALYZE every table
-- has 1,000,000 rows and every filter a fixed selectivity (cost_params.rs); with it, a column no
-- index leads has no distinct count, no histogram and no sample, and filters multiply as if
-- independent (cost.rs constraints_selectivity).
CREATE TABLE t(id INTEGER PRIMARY KEY, a INT, b INT, flag INT, skew INT, u INT);
INSERT INTO t(a, b, flag, skew, u) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 10000) SELECT i % 100, i % 100, i % 2, CASE WHEN i <= 9900 THEN 0 ELSE i END, i FROM n;
CREATE INDEX t_a ON t(a);
CREATE INDEX t_b ON t(b);
CREATE INDEX t_skew ON t(skew);
-- Before ANALYZE: 10,000 rows estimated as 1,000,000.
EXPLAIN QUERY PLAN FORMAT=JSON SELECT * FROM t;
ANALYZE;
SELECT tbl, idx, stat FROM sqlite_stat1 ORDER BY tbl, idx;
-- a and b are equal on every row: a = 1 AND b = 1 keeps 100 rows, estimated as 1 (0.01 x 0.01).
-- counted
SELECT * FROM t WHERE a = 1 AND b = 1;
EXPLAIN QUERY PLAN FORMAT=JSON SELECT * FROM t WHERE a = 1 AND b = 1;
-- flag has two values and no index: flag = 1 keeps half the rows, estimated as a tenth.
-- counted
SELECT * FROM t WHERE flag = 1;
-- skew is 0 on 9,900 rows and distinct on 100: both values estimated alike (no histogram).
-- counted
SELECT * FROM t WHERE skew = 0;
-- counted
SELECT * FROM t WHERE skew = 9950;
-- A range keeps 0.4 of the rows whatever its bound: u > 20000 keeps none.
-- counted
SELECT * FROM t WHERE u > 20000;
