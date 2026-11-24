CREATE TABLE k(id TEXT PRIMARY KEY, v INT) WITHOUT ROWID;
INSERT INTO k VALUES ('a', 1);
INSERT INTO k VALUES (NULL, 1);
SELECT * FROM pragma_integrity_check;
SELECT * FROM k
----
CREATE TABLE k(id TEXT PRIMARY KEY, v INT) STRICT, WITHOUT ROWID;
INSERT INTO k VALUES ('a', 1);
INSERT INTO k VALUES ('b', 'x');
SELECT * FROM pragma_integrity_check;
SELECT * FROM k
----
CREATE TABLE k(id INTEGER PRIMARY KEY, v INT UNIQUE);
INSERT INTO k VALUES (1, 1);
INSERT INTO k VALUES (2, 1);
SELECT * FROM pragma_integrity_check;
SELECT * FROM pragma_table_info('k');
SELECT * FROM k
----
CREATE TABLE k(id INTEGER PRIMARY KEY, v INT UNIQUE);
INSERT INTO k VALUES (1, 1);
INSERT INTO k VALUES (1, 1);
SELECT * FROM pragma_integrity_check;
SELECT * FROM pragma_table_info('k');
SELECT * FROM k
----
CREATE TABLE k(id INTEGER PRIMARY KEY, v INT NOT NULL);
INSERT INTO k VALUES (1, 1);
INSERT INTO k VALUES (2, NULL);
SELECT * FROM pragma_integrity_check;
SELECT integrity_check FROM pragma_integrity_check;
SELECT * FROM pragma_quick_check;
SELECT * FROM k
----
CREATE TABLE k(id INTEGER PRIMARY KEY, v INT NOT NULL);
INSERT INTO k VALUES (1, 1);
UPDATE k SET v = NULL;
SELECT * FROM pragma_integrity_check;
SELECT * FROM k
----
CREATE TABLE k(id INTEGER PRIMARY KEY, v INT NOT NULL);
INSERT INTO k VALUES (1, 1);
SELECT * FROM pragma_integrity_check;
UPDATE k SET v = NULL;
SELECT * FROM pragma_integrity_check;
SELECT * FROM k
