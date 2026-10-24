# diff Wrong: Amphora's answer isn't SQLite's

seed 20261024, state 1241

## Statements

```sql
CREATE TABLE t0(c0 INT, c1 TEXT PRIMARY KEY, c2 DECIMAL(10,2));
INSERT OR IGNORE INTO t0 VALUES (NULL, NULL, NULL);
```

## Queries

```sql
SELECT NULL, count(*) FROM t0 AS a0, t0 AS a1 LEFT JOIN t0 AS a2 ON (a1.c1 <> 255) WHERE ((a2.c2 = 9223372036854775807) ISNULL) GROUP BY NULL;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    NULL, 1
  Amphora:  0 rows
```
