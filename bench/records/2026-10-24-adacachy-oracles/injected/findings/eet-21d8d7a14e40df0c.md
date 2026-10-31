# eet Wrong: an equivalent form gives other rows

seed 20261024, state 373

## Statements

```sql
CREATE TABLE t2(c0 BOOLEAN, c1 INTEGER);
INSERT OR IGNORE INTO t2 VALUES (NULL, 100);
```

## Queries

```sql
SELECT exp((a0.c0 & a0.c1)) FROM t2 AS a0 WHERE ((SELECT count(s908.c0) FROM t2 AS s908 WHERE s908.c1) NOTNULL);
SELECT exp((a0.c0 & a0.c1)) FROM t2 AS a0 WHERE ((((a0.c0 AND (NOT (NOT (a0.c0 NOTNULL)))) OR (NOT (a0.c0 AND (NOT (NOT (a0.c0 NOTNULL)))))) OR ((a0.c0 AND (NOT (NOT (a0.c0 NOTNULL)))) ISNULL)) AND ((SELECT count(s908.c0) FROM t2 AS s908 WHERE s908.c1) NOTNULL));
```

## What differed

```
an equivalent form gives other rows
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
