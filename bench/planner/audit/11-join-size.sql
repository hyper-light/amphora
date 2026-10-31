-- Audit row 11: a join on columns no index leads has a fixed selectivity (0.1, sel_eq_unindexed,
-- constraints.rs) whatever their distinct values: no key or foreign-key containment and no
-- equivalence-class domain (DuckDB, Ebergen). o (5,000 rows) joins c (500) on a key of c.
CREATE TABLE c(ck INT, name TEXT);
CREATE TABLE o(id INTEGER PRIMARY KEY, ck INT);
INSERT INTO c(ck, name) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 500) SELECT i, 'c' || i FROM n;
INSERT INTO o(ck) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 5000) SELECT 1 + i % 500 FROM n;
ANALYZE;
-- counted
SELECT * FROM o JOIN c ON c.ck = o.ck;
EXPLAIN QUERY PLAN FORMAT=JSON SELECT * FROM o JOIN c ON c.ck = o.ck;
-- A GROUP BY's groups are not estimated: o has 500 distinct ck.
EXPLAIN QUERY PLAN FORMAT=JSON SELECT ck, count(*) FROM o GROUP BY ck;
