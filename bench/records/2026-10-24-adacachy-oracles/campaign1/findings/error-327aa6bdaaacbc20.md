# pqs Error: Amphora errors where SQLite finds the pivot: JSON path error near: "0"

seed 11, state 94

## Statements

```sql
CREATE TABLE t0(c0 BOOLEAN PRIMARY KEY, c1 VARCHAR(10));
INSERT INTO t0 VALUES ('0x10', 0);
```

## Queries

```sql
SELECT (jsonb_set(NULL, 0, 3)) IS TRUE, (jsonb_set(NULL, 0, 3)) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 1;
SELECT a0.rowid FROM t0 AS a0 WHERE (jsonb_set(NULL, 0, 3) ISNULL);
```

## What differed

```
Amphora errors where SQLite finds the pivot: JSON path error near: "0"
```
