db dc_group
sql CREATE TABLE o(k TEXT, n INT)
sql CREATE TABLE w(k TEXT)
sql INSERT INTO w VALUES ('a')
q SELECT count(*) FROM o AS b FULL JOIN w AS c ON NULL GROUP BY b.n NOT IN (SELECT s.k FROM w AS s WHERE (b.k IS '1e3'))
q SELECT b.n NOT IN (SELECT s.k FROM w AS s WHERE (b.k IS '1e3')) FROM o AS b FULL JOIN w AS c ON NULL
q SELECT count(*) FROM o AS b FULL JOIN w AS c ON NULL GROUP BY EXISTS (SELECT 1 FROM w AS s WHERE s.k = b.k)
q SELECT count(*) FROM o AS b FULL JOIN w AS c ON NULL GROUP BY (SELECT s.k FROM w AS s WHERE s.k = b.k)
q SELECT count(*) FROM o AS b LEFT JOIN w AS c ON NULL GROUP BY c.k NOT IN (SELECT s.k FROM w AS s WHERE (c.k IS '1e3'))
q SELECT d.c0 FROM (SELECT (CASE WHEN (b.n NOT IN (SELECT s.k FROM w AS s WHERE (b.k IS '1e3'))) THEN b.k ELSE b.n END) AS c0 FROM o AS b FULL JOIN w AS c ON NULL GROUP BY (CASE WHEN (b.n NOT IN (SELECT s.k FROM w AS s WHERE (b.k IS '1e3'))) THEN b.k ELSE b.n END)) AS d
db cursor_none
sql CREATE TABLE w(k TEXT PRIMARY KEY, u INT) WITHOUT ROWID
sql CREATE TABLE o(k TEXT, n INT)
sql INSERT INTO w VALUES ('a', 1)
q SELECT a1.c0 FROM w AS a0, (SELECT CAST((b.k NOT IN (SELECT s.k FROM o AS s WHERE (c.n NOT IN (3)))) AS INTEGER) AS c0 FROM o AS c FULL JOIN w AS b GROUP BY CAST((b.k NOT IN (SELECT s.k FROM o AS s WHERE (c.n NOT IN (3)))) AS INTEGER)) AS a1
db row_between_join
sql CREATE TABLE t0(c0 INTEGER COLLATE NOCASE NOT NULL, c1 BLOB UNIQUE, c2 ANY, c3 ANY COLLATE BINARY PRIMARY KEY) STRICT
sql CREATE TABLE t1(c0 REAL COLLATE NOCASE PRIMARY KEY, c1 INTEGER, c2 INTEGER, c3 NUMERIC)
sql INSERT INTO t0 VALUES (2, x'c3bf', x'c3bf', 'x')
sql INSERT OR IGNORE INTO t1 VALUES ('é', 0, 1, 7)
q SELECT '"x"' FROM t0 AS a0 JOIN t1 AS a1 ON (CASE WHEN (a0.c1 <= NULL) THEN NULL END) JOIN t0 AS a2 ON ((a0.c0, NULL, a2.c2) BETWEEN (1000000000000000.0, 0.1, a0.c0) AND (7, '10', x'41')) WHERE (a2.c2 >= 100)
q SELECT '"x"' FROM t0 AS a0 JOIN t0 AS a2 ON ((a0.c0, a2.c2) BETWEEN (1, a0.c0) AND (7, x'41')) WHERE (a2.c2 >= 100)
q SELECT '"x"' FROM t0 AS a0 JOIN t0 AS a2 ON ((a0.c0, a2.c2) >= (1, a0.c0))
db co
sql CREATE TABLE ev(id INTEGER PRIMARY KEY, at TEXT, v INT)
sql INSERT INTO ev VALUES (1, '2026-01-01 10:00', 5), (2, '2026-01-01 11:00', 3), (3, '2026-01-02 09:00', 9)
q SELECT id, sum(v) OVER (ORDER BY id), lag(v + 1) OVER (ORDER BY date(at), id), coalesce(lag(v + 1) OVER (ORDER BY date(at), id), 0) FROM ev
q SELECT id, sum(v) OVER (ORDER BY id), sum(v + 1) OVER (ORDER BY date(at), id), coalesce(sum(v + 1) OVER (ORDER BY date(at), id), 0) FROM ev
q SELECT id, sum(v) OVER (ORDER BY id), first_value(random()) OVER (ORDER BY v) = first_value(random()) OVER (ORDER BY v) FROM ev
db cp_of_v
sql CREATE TABLE t(k TEXT PRIMARY KEY, v TEXT) WITHOUT ROWID
sql CREATE TABLE lg(a)
sql CREATE TRIGGER g AFTER UPDATE OF v ON t BEGIN INSERT INTO lg VALUES (old.k); END
sql INSERT INTO t VALUES ('a', 'x')
sql UPDATE t SET v = 'b' WHERE k = 'a'
q SELECT * FROM t
q SELECT * FROM lg
