# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 656

## Statements

```sql
CREATE TABLE t2(c0 INTEGER COLLATE RTRIM, c1 DOUBLE COLLATE NOCASE UNIQUE, c2 TEXT COLLATE RTRIM, c3 VARCHAR(10) COLLATE NOCASE);
INSERT INTO t2 VALUES ('true', 0.1, 0.1, 255);
```

## Queries

```sql
SELECT NULL FROM t2 AS a0;
SELECT NULL FROM t2 AS a0 WHERE (NULL - NULL);
SELECT NULL FROM t2 AS a0 WHERE (NOT (NULL - NULL));
SELECT NULL FROM t2 AS a0 WHERE ((NULL - NULL) ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    NULL
  Amphora:  2 rows
    NULL
    NULL
```
