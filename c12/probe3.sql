CREATE TABLE p(id INTEGER PRIMARY KEY, k INT, v INT, s TEXT);
CREATE TABLE q(id INTEGER PRIMARY KEY, k INT, v INT, s TEXT COLLATE NOCASE);
CREATE TABLE f(k INT, v INT, s TEXT);
INSERT INTO p VALUES (1, 1, 2, '1'), (2, 2, 3, 'b'), (3, NULL, 1, 'B'), (4, 4, NULL, NULL), (5, 1, 0, '2');
INSERT INTO q VALUES (1, 2, 2, 'B'), (2, 2, 0, 'a'), (3, 5, 1, '1'), (4, NULL, 4, NULL), (5, 1, 1, 'c');
INSERT INTO f VALUES (1, 2, '2'), (2, 1, 'b'), (2, 5, NULL);
SELECT a0.k, a1.k, a2.k FROM p AS a0 JOIN q AS a1 ON a0.k = a1.k JOIN f AS a2 ON a1.k = a2.v WHERE (SELECT a0.k) IS a2.v;
EXPLAIN SELECT a0.k, a1.k, a2.k FROM p AS a0 JOIN q AS a1 ON a0.k = a1.k JOIN f AS a2 ON a1.k = a2.v WHERE (SELECT a0.k) IS a2.v;
