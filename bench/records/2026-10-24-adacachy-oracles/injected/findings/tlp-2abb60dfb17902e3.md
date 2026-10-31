# tlp Wrong: the query's aggregate isn't its partitions' combined

seed 20261024, state 1247

## Statements

```sql
CREATE TABLE t1(c0 BLOB NOT NULL);
CREATE TABLE t2(c0 INT PRIMARY KEY NOT NULL, c1  UNIQUE);
```

## Queries

```sql
SELECT ((~ 'a') COLLATE BINARY) FROM t2 AS a0, t1 AS a1;
SELECT min(m COLLATE BINARY) FROM (SELECT NULL AS m WHERE 0 UNION ALL SELECT ((~ 'a') COLLATE BINARY) FROM t2 AS a0, t1 AS a1 WHERE (NOT (a0.c1 = (a1.c0 >= a1.c0))) UNION ALL SELECT ((~ 'a') COLLATE BINARY) FROM t2 AS a0, t1 AS a1 WHERE (NOT (NOT (a0.c1 = (a1.c0 >= a1.c0)))) UNION ALL SELECT ((~ 'a') COLLATE BINARY) FROM t2 AS a0, t1 AS a1 WHERE ((NOT (a0.c1 = (a1.c0 >= a1.c0))) ISNULL));
```

## What differed

```
the query's aggregate isn't its partitions' combined
  expected: 0 rows
  Amphora:  1 rows
    NULL
```
