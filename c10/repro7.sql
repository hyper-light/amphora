CREATE TABLE p(s TEXT);
INSERT INTO p VALUES ('1'), ('x');
SELECT p.s, j.key FROM p, json_each('[5,6,7]') AS j WHERE p.s = j.key;
SELECT p.s FROM p WHERE p.s NOT IN (SELECT key FROM json_each('[5,6,7]'));
SELECT p.s FROM p WHERE p.s NOT IN (SELECT j.k FROM (SELECT key AS k FROM json_each('[5,6,7]')) AS j);
SELECT p.s FROM p WHERE p.s IN (SELECT value FROM json_each('[1,6,7]'));
SELECT p.s FROM p LEFT JOIN p AS b ON b.s = p.s WHERE p.s NOT IN (SELECT j.k FROM (SELECT key AS k FROM json_each(json_array(b.s, 1, 2))) AS j);
SELECT count(*) FROM p AS a FULL JOIN p AS b ON a.s = b.s WHERE b.s NOT IN (SELECT j.k FROM (SELECT key AS k FROM json_each(json_array(a.s, 1, 2))) AS j);
