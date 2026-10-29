# tlp Wrong: the query's aggregate isn't its partitions' combined

seed 13, state 11619

## Statements

```sql
CREATE TABLE t2(c0 DOUBLE, c1 DOUBLE);
INSERT INTO t2 VALUES (1, NULL);
INSERT OR IGNORE INTO t2 VALUES ('-1', NULL);
INSERT INTO t2 VALUES (1, 7);
```

## Queries

```sql
SELECT sum(atanh(a0.c0)) FROM t2 AS a0;
SELECT sum(m) FROM (SELECT NULL AS m WHERE 0 UNION ALL SELECT sum(atanh(a0.c0)) FROM t2 AS a0 WHERE a0.c1 UNION ALL SELECT sum(atanh(a0.c0)) FROM t2 AS a0 WHERE (NOT a0.c1) UNION ALL SELECT sum(atanh(a0.c0)) FROM t2 AS a0 WHERE (a0.c1 ISNULL));
```

## What differed

```
the query's aggregate isn't its partitions' combined
  expected: 1 rows
    NULL
  Amphora:  1 rows
    1e999
```
