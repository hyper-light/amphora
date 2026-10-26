-- Audit row 1: every plan is a left-deep pipeline, one table a step; 12 tables or fewer by the
-- dynamic program, more by the greedy search (join.rs GREEDY_JOIN_THRESHOLD), which adds the table
-- of least cost at each step, blind to the rows a filter removes for the steps after it.
-- A fact table f and 12 dimensions, the last filtered (no ANALYZE: every table 1,000,000 rows).
CREATE TABLE f(id INTEGER PRIMARY KEY, k1 INT, k2 INT, k3 INT, k4 INT, k5 INT, k6 INT, k7 INT, k8 INT, k9 INT, k10 INT, k11 INT, k12 INT, v INT);
CREATE TABLE d1(id INTEGER PRIMARY KEY, v INT, w INT);
CREATE TABLE d2(id INTEGER PRIMARY KEY, v INT, w INT);
CREATE TABLE d3(id INTEGER PRIMARY KEY, v INT, w INT);
CREATE TABLE d4(id INTEGER PRIMARY KEY, v INT, w INT);
CREATE TABLE d5(id INTEGER PRIMARY KEY, v INT, w INT);
CREATE TABLE d6(id INTEGER PRIMARY KEY, v INT, w INT);
CREATE TABLE d7(id INTEGER PRIMARY KEY, v INT, w INT);
CREATE TABLE d8(id INTEGER PRIMARY KEY, v INT, w INT);
CREATE TABLE d9(id INTEGER PRIMARY KEY, v INT, w INT);
CREATE TABLE d10(id INTEGER PRIMARY KEY, v INT, w INT);
CREATE TABLE d11(id INTEGER PRIMARY KEY, v INT, w INT);
CREATE TABLE d12(id INTEGER PRIMARY KEY, v INT, w INT);
-- 12 tables (the dynamic program): the filtered dimension d11 is read right after f.
EXPLAIN QUERY PLAN FORMAT=JSON SELECT count(*) FROM f JOIN d1 ON d1.id = f.k1 JOIN d2 ON d2.id = f.k2 JOIN d3 ON d3.id = f.k3 JOIN d4 ON d4.id = f.k4 JOIN d5 ON d5.id = f.k5 JOIN d6 ON d6.id = f.k6 JOIN d7 ON d7.id = f.k7 JOIN d8 ON d8.id = f.k8 JOIN d9 ON d9.id = f.k9 JOIN d10 ON d10.id = f.k10 JOIN d11 ON d11.id = f.k11 WHERE d11.v = 7;
-- 13 tables (the greedy search): the filtered dimension d12 is read last.
EXPLAIN QUERY PLAN FORMAT=JSON SELECT count(*) FROM f JOIN d1 ON d1.id = f.k1 JOIN d2 ON d2.id = f.k2 JOIN d3 ON d3.id = f.k3 JOIN d4 ON d4.id = f.k4 JOIN d5 ON d5.id = f.k5 JOIN d6 ON d6.id = f.k6 JOIN d7 ON d7.id = f.k7 JOIN d8 ON d8.id = f.k8 JOIN d9 ON d9.id = f.k9 JOIN d10 ON d10.id = f.k10 JOIN d11 ON d11.id = f.k11 JOIN d12 ON d12.id = f.k12 WHERE d12.v = 7;
-- The same 13 tables in the order the dynamic program would choose, forced by CROSS JOIN.
EXPLAIN QUERY PLAN FORMAT=JSON SELECT count(*) FROM f CROSS JOIN d12 CROSS JOIN d1 CROSS JOIN d2 CROSS JOIN d3 CROSS JOIN d4 CROSS JOIN d5 CROSS JOIN d6 CROSS JOIN d7 CROSS JOIN d8 CROSS JOIN d9 CROSS JOIN d10 CROSS JOIN d11 WHERE d1.id = f.k1 AND d2.id = f.k2 AND d3.id = f.k3 AND d4.id = f.k4 AND d5.id = f.k5 AND d6.id = f.k6 AND d7.id = f.k7 AND d8.id = f.k8 AND d9.id = f.k9 AND d10.id = f.k10 AND d11.id = f.k11 AND d12.id = f.k12 AND d12.v = 7;
-- Two pairs of tables that a bushy plan would join apart, (a JOIN b) JOIN (c JOIN d): the plan is
-- one loop inside another, a left-deep tree.
CREATE TABLE a(id INTEGER PRIMARY KEY, x INT);
CREATE TABLE b(id INTEGER PRIMARY KEY, x INT, y INT);
CREATE TABLE c(id INTEGER PRIMARY KEY, y INT, z INT);
CREATE TABLE d(id INTEGER PRIMARY KEY, z INT);
EXPLAIN QUERY PLAN SELECT count(*) FROM a JOIN b ON b.x = a.x JOIN c ON c.y = b.y JOIN d ON d.z = c.z;
