# tlp Wrong: the query's rows aren't the UNION of its partitions'

seed 20261024, state 1052

## Statements

```sql
CREATE TABLE t0(c0 INT COLLATE BINARY PRIMARY KEY);
INSERT OR IGNORE INTO t0 VALUES (1.5);
CREATE TABLE t1(c0 INTEGER, c1 INTEGER COLLATE BINARY PRIMARY KEY NOT NULL, c2 DECIMAL(10,2), c3 );
```

## Queries

```sql
SELECT CAST(CAST(a0.c1 AS INTEGER) AS TEXT) FROM t1 AS a0, t1 AS a1 FULL JOIN t0 AS a2 ON ((a1.c3 = 0.1) == octet_length(-9223372036854775808));
SELECT CAST(CAST(a0.c1 AS INTEGER) AS TEXT) FROM t1 AS a0, t1 AS a1 FULL JOIN t0 AS a2 ON ((a1.c3 = 0.1) == octet_length(-9223372036854775808)) WHERE (a1.c2 <> ((a2.c0 < x'00') AND (a0.c2 > a1.c1))) UNION SELECT CAST(CAST(a0.c1 AS INTEGER) AS TEXT) FROM t1 AS a0, t1 AS a1 FULL JOIN t0 AS a2 ON ((a1.c3 = 0.1) == octet_length(-9223372036854775808)) WHERE (NOT (a1.c2 <> ((a2.c0 < x'00') AND (a0.c2 > a1.c1)))) UNION SELECT CAST(CAST(a0.c1 AS INTEGER) AS TEXT) FROM t1 AS a0, t1 AS a1 FULL JOIN t0 AS a2 ON ((a1.c3 = 0.1) == octet_length(-9223372036854775808)) WHERE ((a1.c2 <> ((a2.c0 < x'00') AND (a0.c2 > a1.c1))) ISNULL);
```

## What differed

```
the query's rows aren't the UNION of its partitions'
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
