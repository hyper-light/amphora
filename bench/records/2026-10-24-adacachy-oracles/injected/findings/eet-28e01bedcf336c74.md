# eet Wrong: an equivalent form gives other rows

seed 20261024, state 946

## Statements

```sql
CREATE TABLE t1(c0 , c1 VARCHAR(10), c2 INTEGER NOT NULL);
INSERT INTO t1 VALUES (0.1, NULL, -2.5);
```

## Queries

```sql
SELECT CAST((a1.c2 BETWEEN a0.c2 AND 2) AS REAL) FROM t1 AS a0 RIGHT JOIN t1 AS a1 ON ((1 ISNULL) AND (NOT a1.c2)) WHERE (((a0.c0 | a1.c1) & asin(a1.c0)) ISNULL);
SELECT CAST((a1.c2 BETWEEN a0.c2 AND 2) AS REAL) FROM t1 AS a0 RIGHT JOIN t1 AS a1 ON ((1 ISNULL) AND (NOT a1.c2)) WHERE ((CASE WHEN (((a0.c0 NOTNULL) AND (NOT (a0.c0 NOTNULL))) AND ((a0.c0 NOTNULL) NOTNULL)) THEN 1 ELSE ((a0.c0 | a1.c1) & asin(a1.c0)) END) ISNULL);
```

## What differed

```
an equivalent form gives other rows
  expected: 0 rows
  Amphora:  1 rows
    NULL
```
