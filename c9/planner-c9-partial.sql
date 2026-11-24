CREATE TABLE a(id INTEGER PRIMARY KEY, x INT)
CREATE TABLE b(id INTEGER PRIMARY KEY, aid INT, flag INT, v INT)
CREATE INDEX b_p ON b(aid) WHERE flag = 1
CREATE TABLE c(id INTEGER PRIMARY KEY, v INT)
INSERT INTO a VALUES (1, 1), (2, 2), (3, 3)
INSERT INTO b VALUES (1, 1, 1, 5), (2, 1, 0, 6), (3, 3, 1, 7), (4, 3, 0, 8)
INSERT INTO c VALUES (1, 1), (2, 2), (3, 3)
--
SELECT a.id, b.id, c.id FROM a JOIN b ON b.aid = a.id LEFT JOIN c ON b.flag = 1 AND c.id = b.id
SELECT b.id, c.id FROM b LEFT JOIN c ON b.flag = 1 AND c.id = b.id WHERE b.aid = 1
