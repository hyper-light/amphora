# tlp Wrong: the query's rows aren't the UNION of its partitions'

seed 20261024, state 1246

## Statements

```sql
CREATE TABLE t1(c0 BLOB NOT NULL, c1 BLOB COLLATE RTRIM);
CREATE TABLE t2(c0 DOUBLE);
INSERT INTO t2 VALUES (255);
```

## Queries

```sql
SELECT ('ABC' COLLATE BINARY) FROM t2 AS a0, t2 AS a1 LEFT JOIN t1 AS a2 ON ((a0.c0 % 'true') <> CAST(a2.c1 AS INTEGER)) GROUP BY ('ABC' COLLATE BINARY);
SELECT ('ABC' COLLATE BINARY) FROM t2 AS a0, t2 AS a1 LEFT JOIN t1 AS a2 ON ((a0.c0 % 'true') <> CAST(a2.c1 AS INTEGER)) WHERE (((a2.c0 % 1000000000000000.0) % jsonb_patch(a1.c0, a2.c1)) ISNULL) GROUP BY ('ABC' COLLATE BINARY) UNION SELECT ('ABC' COLLATE BINARY) FROM t2 AS a0, t2 AS a1 LEFT JOIN t1 AS a2 ON ((a0.c0 % 'true') <> CAST(a2.c1 AS INTEGER)) WHERE (NOT (((a2.c0 % 1000000000000000.0) % jsonb_patch(a1.c0, a2.c1)) ISNULL)) GROUP BY ('ABC' COLLATE BINARY) UNION SELECT ('ABC' COLLATE BINARY) FROM t2 AS a0, t2 AS a1 LEFT JOIN t1 AS a2 ON ((a0.c0 % 'true') <> CAST(a2.c1 AS INTEGER)) WHERE ((((a2.c0 % 1000000000000000.0) % jsonb_patch(a1.c0, a2.c1)) ISNULL) ISNULL) GROUP BY ('ABC' COLLATE BINARY);
```

## What differed

```
the query's rows aren't the UNION of its partitions'
  expected: 1 rows
    'ABC'
  Amphora:  0 rows
```
