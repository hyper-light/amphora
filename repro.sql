CREATE TABLE item(id INTEGER PRIMARY KEY, qty INT, last INT) STRICT;
INSERT INTO item VALUES (10, 1, 0);
INSERT INTO item VALUES (10, 5, 0) ON CONFLICT(id) DO UPDATE SET qty = item.qty + excluded.qty, last = excluded.id;
SELECT id, qty, last, typeof(last) FROM item;
EXPLAIN INSERT INTO item VALUES (10, 5, 0) ON CONFLICT(id) DO UPDATE SET last = excluded.id;
----
CREATE TABLE item(id INTEGER PRIMARY KEY, qty INT, last INT);
INSERT INTO item VALUES (10, 1, 0);
INSERT INTO item VALUES (10, 5, 0) ON CONFLICT(id) DO UPDATE SET qty = item.qty + excluded.qty, last = excluded.id;
SELECT id, qty, last, typeof(last) FROM item;
EXPLAIN INSERT INTO item VALUES (10, 5, 0) ON CONFLICT(id) DO UPDATE SET last = excluded.id;
----
CREATE TABLE item(id INTEGER PRIMARY KEY, qty INT) STRICT;
INSERT INTO item VALUES (10, 1);
INSERT INTO item VALUES (10, 5) ON CONFLICT(id) DO UPDATE SET qty = excluded.qty WHERE excluded.id = 10;
SELECT * FROM item;
----
CREATE TABLE item(id INTEGER PRIMARY KEY, qty INT) STRICT;
INSERT INTO item VALUES (10, 1);
INSERT INTO item VALUES (10, 5) ON CONFLICT(id) DO UPDATE SET id = excluded.id + 1;
SELECT * FROM item;
----
CREATE TABLE item(id INTEGER PRIMARY KEY, qty INT) STRICT;
INSERT INTO item VALUES (10, 1);
INSERT INTO item VALUES (10, 5) ON CONFLICT(id) DO UPDATE SET id = excluded.id, qty = excluded.rowid;
SELECT * FROM item;
----
CREATE TABLE kv(id INTEGER PRIMARY KEY, name TEXT UNIQUE, n INT) STRICT;
INSERT INTO kv VALUES (1, 'a', 0);
INSERT INTO kv VALUES (7, 'a', 5) ON CONFLICT(name) DO UPDATE SET id = excluded.id, n = excluded.n;
SELECT * FROM kv;
----
CREATE TABLE kv(id INTEGER PRIMARY KEY, name TEXT UNIQUE, n INT) STRICT;
INSERT INTO kv VALUES (1, 'a', 0);
INSERT INTO kv(name, n) VALUES ('a', 5) ON CONFLICT(name) DO UPDATE SET n = excluded.id;
SELECT * FROM kv;
----
CREATE TABLE kv(id INTEGER PRIMARY KEY, name TEXT UNIQUE, n INT);
INSERT INTO kv VALUES (1, 'a', 0);
INSERT INTO kv(name, n) VALUES ('a', 5) ON CONFLICT(name) DO UPDATE SET n = excluded.id;
SELECT * FROM kv;
----
CREATE TABLE kv(id INTEGER PRIMARY KEY, name TEXT UNIQUE, n INT);
INSERT INTO kv VALUES (1, 'a', 0);
INSERT INTO kv VALUES (NULL, 'a', 5) ON CONFLICT(name) DO UPDATE SET n = excluded.id;
SELECT * FROM kv;
----
CREATE TABLE kv(id INTEGER PRIMARY KEY, name TEXT UNIQUE, n INT) STRICT;
INSERT INTO kv VALUES (1, 'a', 0);
INSERT INTO kv(rowid, name, n) VALUES (9, 'a', 5) ON CONFLICT(name) DO UPDATE SET n = excluded.id;
SELECT * FROM kv;
----
CREATE TABLE g(id INTEGER PRIMARY KEY, a INT, b INT GENERATED ALWAYS AS (id * 2 + a) VIRTUAL, c INT) STRICT;
INSERT INTO g(id, a, c) VALUES (3, 1, 0);
INSERT INTO g(id, a, c) VALUES (3, 4, 0) ON CONFLICT(id) DO UPDATE SET c = excluded.b;
SELECT * FROM g;
----
CREATE TABLE g(id INTEGER PRIMARY KEY, a INT, b INT GENERATED ALWAYS AS (id * 2 + a) VIRTUAL, c INT);
INSERT INTO g(id, a, c) VALUES (3, 1, 0);
INSERT INTO g(id, a, c) VALUES (3, 4, 0) ON CONFLICT(id) DO UPDATE SET c = excluded.b;
SELECT * FROM g;
----
CREATE TABLE price(id INTEGER PRIMARY KEY, amount REAL) STRICT;
CREATE TABLE audit(amount);
CREATE TRIGGER g BEFORE INSERT ON price BEGIN INSERT INTO audit VALUES (new.amount); END;
INSERT INTO price VALUES (1, 100);
SELECT amount, typeof(amount) FROM audit;
EXPLAIN INSERT INTO price VALUES (2, 100);
----
CREATE TABLE n(id INTEGER PRIMARY KEY, v INTEGER) STRICT;
CREATE TABLE audit(v);
CREATE TRIGGER g BEFORE UPDATE ON n BEGIN INSERT INTO audit VALUES (new.v); END;
INSERT INTO n VALUES (1, 2);
UPDATE n SET v = '7' WHERE id = 1;
SELECT v, typeof(v) FROM audit;
SELECT * FROM n;
EXPLAIN UPDATE n SET v = '7' WHERE id = 1;
