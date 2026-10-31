# norec Error: the unoptimized form errors on Amphora, not on SQLite: Parse error: Unexpected end of input

seed 12, state 1837

## Statements

```sql
CREATE TABLE t0(c0 NUMERIC NOT NULL, c1 BOOLEAN, c2 INT COLLATE NOCASE, c3 NUMERIC);
INSERT INTO t0 VALUES (x'', 3, 0.0, 100);
```

## Queries

```sql
SELECT 1 FROM t0 AS a0 WHERE (NULL AND jsonb_patch(a0.c0, 0));
SELECT (CASE WHEN (NULL AND jsonb_patch(a0.c0, 0)) THEN 1 ELSE 0 END) FROM t0 AS a0;
```

## What differed

```
the unoptimized form errors on Amphora, not on SQLite: Parse error: Unexpected end of input
```
