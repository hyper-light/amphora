# tlp Wrong: the query's rows aren't its three partitions' (p, NOT p, p IS NULL)

seed 20261024, state 1196

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE);
INSERT INTO t0 VALUES (NULL);
```

## Queries

```sql
SELECT 9223372036854775807 FROM t0 AS a0;
SELECT 9223372036854775807 FROM t0 AS a0 WHERE (NULL NOT LIKE 'A%');
SELECT 9223372036854775807 FROM t0 AS a0 WHERE (NOT (NULL NOT LIKE 'A%'));
SELECT 9223372036854775807 FROM t0 AS a0 WHERE ((NULL NOT LIKE 'A%') ISNULL);
```

## What differed

```
the query's rows aren't its three partitions' (p, NOT p, p IS NULL)
  expected: 1 rows
    9223372036854775807
  Amphora:  2 rows
    9223372036854775807
    9223372036854775807
```
