-- Audit row 10: no adaptive execution: a plan runs as chosen whatever rows it meets (no switch
-- from nested loops to a hash join, no re-optimization, no sideways filters). Statistics taken
-- when the tables held 10 rows each, then each grown to 3,000: the plan is a nested loop over a
-- scan of the inner table for every outer row.
CREATE TABLE p(id INTEGER PRIMARY KEY, k INT);
CREATE TABLE q(id INTEGER PRIMARY KEY, k INT);
INSERT INTO p(k) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 10) SELECT i FROM n;
INSERT INTO q(k) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 10) SELECT i FROM n;
ANALYZE;
INSERT INTO p(k) WITH RECURSIVE n(i) AS (SELECT 11 UNION ALL SELECT i + 1 FROM n WHERE i < 3000) SELECT i FROM n;
INSERT INTO q(k) WITH RECURSIVE n(i) AS (SELECT 11 UNION ALL SELECT i + 1 FROM n WHERE i < 3000) SELECT i FROM n;
EXPLAIN QUERY PLAN FORMAT=JSON SELECT count(*) FROM p JOIN q ON q.k = p.k;
-- counted
SELECT count(*) FROM p JOIN q ON q.k = p.k;
