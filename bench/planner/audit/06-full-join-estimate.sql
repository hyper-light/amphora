-- Audit row 6: an outer join's estimate counts its left input's unmatched rows (a Poisson guess,
-- join.rs rows_after_join) but a FULL JOIN's estimate leaves out the right table's unmatched rows.
-- l has 100 rows, r 1,000, and no key matches: LEFT gives 100 rows, RIGHT 1,000, FULL 1,100.
CREATE TABLE l(id INTEGER PRIMARY KEY, k INT);
CREATE TABLE r(id INTEGER PRIMARY KEY, k INT);
INSERT INTO l(k) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 100) SELECT i FROM n;
INSERT INTO r(k) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 1000) SELECT 1000 + i FROM n;
CREATE INDEX r_k ON r(k);
ANALYZE;
-- counted
SELECT * FROM l LEFT JOIN r ON r.k = l.k;
-- counted
SELECT * FROM l RIGHT JOIN r ON r.k = l.k;
-- counted
SELECT * FROM l FULL JOIN r ON r.k = l.k;
EXPLAIN QUERY PLAN FORMAT=JSON SELECT * FROM l FULL JOIN r ON r.k = l.k;
EXPLAIN QUERY PLAN FORMAT=JSON SELECT * FROM l JOIN r ON r.k = l.k;
