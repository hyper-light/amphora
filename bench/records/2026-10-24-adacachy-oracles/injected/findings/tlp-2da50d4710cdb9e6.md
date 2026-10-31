# tlp Wrong: the query's rows aren't the UNION of its partitions'

seed 20261024, state 1526

## Statements

```sql
CREATE TABLE t0(c0 INTEGER COLLATE RTRIM);
INSERT INTO t0 VALUES ('%');
```

## Queries

```sql
SELECT NULL FROM t0 AS a0 FULL JOIN t0 AS a1 ON 'a ';
SELECT NULL FROM t0 AS a0 FULL JOIN t0 AS a1 ON 'a ' WHERE -9223372036854775808 UNION SELECT NULL FROM t0 AS a0 FULL JOIN t0 AS a1 ON 'a ' WHERE (NOT -9223372036854775808) UNION SELECT NULL FROM t0 AS a0 FULL JOIN t0 AS a1 ON 'a ' WHERE (-9223372036854775808 ISNULL);
```

## What differed

```
the query's rows aren't the UNION of its partitions'
  expected: 2 rows
    NULL
    NULL
  Amphora:  1 rows
    NULL
```
