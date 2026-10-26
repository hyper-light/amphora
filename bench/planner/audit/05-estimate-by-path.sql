-- Audit row 5: a join's row estimate is the access path's rows per outer row times its filters
-- (join.rs rows_after_join, access_method.rs estimated_rows_per_outer_row), not a property of the
-- set of tables joined: the same tables and terms are estimated differently by the path chosen.
CREATE TABLE r(id INTEGER PRIMARY KEY, k INT, v INT);
CREATE TABLE s(id INTEGER PRIMARY KEY, k INT, v INT);
INSERT INTO r(k, v) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 2000) SELECT i % 200, i % 7 FROM n;
INSERT INTO s(k, v) WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 2000) SELECT i % 200, i % 7 FROM n;
CREATE INDEX s_k ON s(k);
ANALYZE;
-- s reached by its index on k: rows per outer row from sqlite_stat1.
-- counted
SELECT * FROM r JOIN s ON s.k = r.k;
EXPLAIN QUERY PLAN FORMAT=JSON SELECT * FROM r JOIN s ON s.k = r.k;
-- The same join with s NOT INDEXED: the same term as a filter, its fixed selectivity.
EXPLAIN QUERY PLAN FORMAT=JSON SELECT * FROM r JOIN s NOT INDEXED ON s.k = r.k;
-- The same join read s first (CROSS JOIN keeps the order written).
EXPLAIN QUERY PLAN FORMAT=JSON SELECT * FROM s CROSS JOIN r WHERE s.k = r.k;
-- Campaign 2's AG finding cert-70d3e0baf8df5914 (bench/records/2026-10-24-adacachy-oracles): a
-- conjunct added to a FULL JOIN's WHERE raised its estimate.
CREATE TABLE t0(c0 REAL PRIMARY KEY NOT NULL);
CREATE TABLE t1(c0 VARCHAR(10), c1 TEXT PRIMARY KEY);
EXPLAIN QUERY PLAN FORMAT=JSON SELECT NULL FROM t1 AS a0 FULL JOIN t0 AS a1 ON ((a1.c0 > 1000000000000000.0) AND (a0.c1 >= a1.c0)) WHERE (a1.c0 IN (NULL, 1.5));
EXPLAIN QUERY PLAN FORMAT=JSON SELECT NULL FROM t1 AS a0 FULL JOIN t0 AS a1 ON ((a1.c0 > 1000000000000000.0) AND (a0.c1 >= a1.c0)) WHERE ((a1.c0 IN (NULL, 1.5)) AND a0.c1);
