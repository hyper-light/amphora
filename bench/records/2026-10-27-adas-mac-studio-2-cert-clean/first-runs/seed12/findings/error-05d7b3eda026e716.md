# pqs Error: Amphora errors where SQLite finds the pivot: JSON path error near: "1.0"

seed 12, state 28592

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE, c1 INT PRIMARY KEY);
INSERT INTO t0 VALUES ('1', NULL);
INSERT OR IGNORE INTO t0 VALUES (1, 9223372036854775807), (NULL, '10');
INSERT OR IGNORE INTO t0 VALUES ('1', 2), (-2.5, -1);
INSERT INTO t0 VALUES (' ', -1e-308), (2.0, 9007199254740993), (1, ' ');
UPDATE t0 SET c1 = NULL WHERE c1 IS NOT NULL;
```

## Queries

```sql
SELECT ((EXISTS (SELECT 1 FROM t0 AS s635 WHERE (('0x10' BETWEEN NULL AND 1.5) BETWEEN jsonb_extract(s635.c0, s635.c0, '') AND s635.c1)))) IS TRUE, ((EXISTS (SELECT 1 FROM t0 AS s635 WHERE (('0x10' BETWEEN NULL AND 1.5) BETWEEN jsonb_extract(s635.c0, s635.c0, '') AND s635.c1)))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 8;
SELECT a0.rowid FROM t0 AS a0 WHERE (NOT (EXISTS (SELECT 1 FROM t0 AS s635 WHERE (('0x10' BETWEEN NULL AND 1.5) BETWEEN jsonb_extract(s635.c0, s635.c0, '') AND s635.c1))));
```

## What differed

```
Amphora errors where SQLite finds the pivot: JSON path error near: "1.0"
```
