CREATE TABLE src(a, b);
CREATE TABLE kv(id INTEGER PRIMARY KEY, n INT, last INT, seen ANY) STRICT;
CREATE TRIGGER g AFTER INSERT ON src BEGIN INSERT INTO kv(id, n) VALUES (new.a, new.b) ON CONFLICT(id) DO UPDATE SET n = n + excluded.n, last = excluded.id, seen = typeof(excluded.n) WHERE excluded.id = new.a; END;
INSERT INTO src VALUES (1, 5), (1, 7), (2, 3), (1, 9);
SELECT * FROM kv;
----
CREATE TABLE src(a, b);
CREATE TABLE kv(id INTEGER PRIMARY KEY, name TEXT UNIQUE, last INT);
CREATE TRIGGER g AFTER INSERT ON src BEGIN INSERT INTO kv(name) VALUES (new.a) ON CONFLICT(name) DO UPDATE SET last = excluded.id + new.b; END;
INSERT INTO src VALUES ('x', 5), ('x', 7), ('y', 3), ('x', 9);
SELECT * FROM kv;
----
CREATE TABLE t(id INTEGER PRIMARY KEY, c INT, d INT) STRICT;
INSERT INTO t VALUES (1, 1, 1);
INSERT INTO t AS x VALUES (1, 5, 6) ON CONFLICT(id) DO UPDATE SET c = x.c + excluded.id, d = excluded.d + x.id + x.rowid;
SELECT * FROM t;
INSERT INTO t VALUES (1, 8, 9) ON CONFLICT(id) DO UPDATE SET c = excluded.c RETURNING id, c, d;
----
CREATE TABLE excluded(id INTEGER PRIMARY KEY, c INT) STRICT;
INSERT INTO excluded VALUES (1, 1);
INSERT INTO excluded VALUES (1, 5) ON CONFLICT(id) DO UPDATE SET c = excluded.c + 1;
SELECT * FROM excluded;
----
CREATE TABLE t(id INTEGER PRIMARY KEY, c INT) STRICT;
CREATE TABLE u(id INTEGER PRIMARY KEY, c INT, d INT) STRICT;
INSERT INTO t VALUES (1, 10), (2, 20);
INSERT INTO u VALUES (1, 0, 0), (2, 0, 0);
INSERT INTO u(id, c) SELECT id, c FROM t WHERE true ON CONFLICT(id) DO UPDATE SET c = excluded.c, d = excluded.id;
SELECT * FROM u;
INSERT INTO u(id, c) SELECT id + 0, c FROM u WHERE true ON CONFLICT(id) DO UPDATE SET d = d + excluded.id * 100;
SELECT * FROM u;
