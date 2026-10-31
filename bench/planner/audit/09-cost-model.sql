-- Audit row 9: cost is page reads, seeks and per-row work in one unit (cost.rs, cost_params.rs);
-- a table's pages are its rows over a fixed 50 rows a page (rows_per_table_page), not the pages it
-- holds. JOB found estimates, not costs, decided plans (f12, graded D for Amphora's counterclaim).
-- Two tables of 2,000 rows each, one row narrow, one of 2,000 bytes: the same cost to scan.
CREATE TABLE narrow(id INTEGER PRIMARY KEY, v INT);
CREATE TABLE wide(id INTEGER PRIMARY KEY, v INT, pad TEXT);
INSERT INTO narrow(v) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 2000) SELECT i FROM n;
-- The file's pages with narrow's rows, then with wide's too.
PRAGMA page_count;
INSERT INTO wide(v, pad) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 2000) SELECT i, printf('%2000s', i) FROM n;
ANALYZE;
PRAGMA page_count;
EXPLAIN QUERY PLAN FORMAT=JSON SELECT sum(v) FROM narrow;
EXPLAIN QUERY PLAN FORMAT=JSON SELECT sum(v) FROM wide;
