CREATE TABLE a(id INTEGER PRIMARY KEY, total REAL, code TEXT UNIQUE, note TEXT, k INT)
CREATE INDEX a_total ON a(total)
CREATE INDEX a_k ON a(k)
CREATE TABLE b(id INTEGER PRIMARY KEY, aid INT, flag INT, v INT)
CREATE INDEX b_v ON b(v)
CREATE INDEX b_flag ON b(flag)
CREATE TABLE c(id INTEGER PRIMARY KEY, v INT)
INSERT INTO a VALUES (1, 255, '255', NULL, 1), (2, 50, '050', NULL, 1), (3, 300, '099', NULL, 2)
INSERT INTO b VALUES (1, 1, 1, 5), (2, 1, 0, 6), (3, 3, 1, 7), (4, 3, 0, 8)
INSERT INTO c VALUES (1, 1), (2, 2), (3, 3)
--
SELECT a.id, b.id FROM a LEFT JOIN b ON a.code = '255' AND b.id = a.id WHERE a.total > 100 AND a.note IS NULL
SELECT a.id, c.id FROM a LEFT JOIN c ON a.code = '255' WHERE a.total > 100 AND a.note IS NULL
SELECT a.id, c.id FROM a LEFT JOIN c ON a.k = 1 WHERE a.total > 100 AND a.note IS NULL
SELECT a.id, c.id FROM a LEFT JOIN c ON a.code = '255' AND a.k = 1 WHERE a.total > 100 AND a.note IS NULL
SELECT a.id, c.id FROM c RIGHT JOIN a ON a.code = '255' AND c.id = a.id WHERE a.total > 100 AND a.note IS NULL
SELECT a.id, c.id FROM a FULL JOIN c ON a.code = '255' AND c.id = a.id WHERE a.total > 100 AND a.note IS NULL
SELECT a.id, b.id, c.id FROM a JOIN b ON b.aid = a.id LEFT JOIN c ON b.v = 5 AND c.id = b.id WHERE b.flag = 1 AND b.id > 0
SELECT a.id, b.id, c.id FROM a JOIN b ON b.aid = a.id LEFT JOIN c ON b.v > 6 WHERE b.flag >= 0 AND b.id > 0 AND c.id IS NULL
SELECT a.id, c.id FROM a LEFT JOIN c ON (a.code = '255' OR a.k = 2) AND c.id = a.id WHERE a.total > 100 AND a.note IS NULL
