# pqs Error: Amphora errors where SQLite finds the pivot: Parse error: malformed JSON

seed 27004, state 19211

## Statements

```sql
CREATE TABLE t0(c0 TEXT, c1 DECIMAL(10,2), c2 TEXT);
INSERT OR REPLACE INTO t0 VALUES ('a ', -2.5, 100), ('0', x'41', '1.5');
INSERT OR REPLACE INTO t0 VALUES (2, 'A', -2.5), ('abc', 3, 'abc'), (-1e-308, NULL, NULL);
CREATE VIEW v0 AS SELECT 10 AS c0 FROM t0 AS b56x0 WHERE (json(b56x0.c2) > (b56x0.c2 / -9223372036854775808));
```

## Queries

```sql
SELECT ((SELECT sum(s818.c0) FROM v0 AS s818 WHERE (s818.c0 ISNULL))) IS TRUE, ((SELECT sum(s818.c0) FROM v0 AS s818 WHERE (s818.c0 ISNULL))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 5;
SELECT a0.rowid FROM t0 AS a0 WHERE ((SELECT sum(s818.c0) FROM v0 AS s818 WHERE (s818.c0 ISNULL)) ISNULL);
```

## What differed

```
Amphora errors where SQLite finds the pivot: Parse error: malformed JSON
```
