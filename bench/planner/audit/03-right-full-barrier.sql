-- Audit row 3: a RIGHT or FULL JOIN is a reordering barrier both ways (join.rs
-- required_lhs_by_table), where a conflict detector (CD-C) allows every reordering that keeps the
-- result. (big RIGHT JOIN mid ON ...) JOIN tiny ON tiny.id = mid.id is big RIGHT JOIN (mid JOIN
-- tiny): tiny's term reads only mid, so tiny may be read first. No ANALYZE.
CREATE TABLE big(id INTEGER PRIMARY KEY, k INT);
CREATE TABLE mid(id INTEGER PRIMARY KEY, k INT);
CREATE TABLE tiny(id INTEGER PRIMARY KEY, k INT);
-- Inner joins: tiny's one row (id = 5) first.
EXPLAIN QUERY PLAN FORMAT=JSON SELECT count(*) FROM big JOIN mid ON mid.k = big.k JOIN tiny ON tiny.id = mid.id WHERE tiny.id = 5;
-- The same tables with mid's rows kept by a RIGHT JOIN: big and mid stay ahead of tiny.
EXPLAIN QUERY PLAN FORMAT=JSON SELECT count(*) FROM big RIGHT JOIN mid ON mid.k = big.k JOIN tiny ON tiny.id = mid.id WHERE tiny.id = 5;
-- Its equivalent written with the RIGHT JOIN last, as a LEFT JOIN: tiny first again.
EXPLAIN QUERY PLAN FORMAT=JSON SELECT count(*) FROM mid JOIN tiny ON tiny.id = mid.id LEFT JOIN big ON mid.k = big.k WHERE tiny.id = 5;
