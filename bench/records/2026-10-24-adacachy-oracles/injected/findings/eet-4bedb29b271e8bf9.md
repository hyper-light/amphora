# eet Wrong: an equivalent form gives other rows

seed 20261024, state 729

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE, c1  COLLATE BINARY PRIMARY KEY, c2 REAL, c3 BOOLEAN);
INSERT INTO t0 VALUES (2, -2.5, '0', 1000000000000000.0);
```

## Queries

```sql
SELECT (a0.c1 % (a0.c1 COLLATE RTRIM)) FROM t0 AS a0 WHERE (NOT (json_extract('{"a":1,"b":[2,3]}', '$[0]') NOT GLOB 'abc'));
SELECT (a0.c1 % (a0.c1 COLLATE RTRIM)) FROM t0 AS a0 WHERE ((((NULL AND (CASE WHEN (((a0.c0 <> NULL) OR (NOT (a0.c0 <> NULL))) OR ((a0.c0 <> NULL) ISNULL)) THEN (a0.c1 <> a0.c1) ELSE 1.5 END)) OR (NOT (NULL AND (a0.c1 <> a0.c1)))) OR (((CASE WHEN ((a0.c3 < a0.c2) OR (a0.c0 BETWEEN a0.c1 AND 'a')) THEN NULL ELSE NULL END) AND (a0.c1 <> a0.c1)) ISNULL)) AND (NOT (json_extract('{"a":1,"b":[2,3]}', '$[0]') NOT GLOB 'abc')));
```

## What differed

```
an equivalent form gives other rows
  expected: 1 rows
    0.0
  Amphora:  0 rows
```
