# eet Wrong: an equivalent form gives other rows

seed 20261024, state 244

## Statements

```sql
CREATE TABLE t0(c0 VARCHAR(10), c1 BOOLEAN COLLATE BINARY, c2 DECIMAL(10,2) NOT NULL);
INSERT INTO t0 VALUES (NULL, '', 0);
```

## Queries

```sql
SELECT (+ x'00') FROM t0 AS a0;
SELECT (+ (CASE WHEN ((((a0.c0 IN (a0.c1, 1, NULL)) AND (a0.c0 NOTNULL)) OR (NOT ((a0.c0 IN (a0.c1, 1, NULL)) AND (a0.c0 NOTNULL)))) OR (((a0.c0 IN (a0.c1, 1, NULL)) AND (a0.c0 NOTNULL)) ISNULL)) THEN (CASE WHEN (NOT ((((a0.c1 <= NULL) AND (NOT (a0.c1 <= NULL))) AND ((a0.c1 <= NULL) NOTNULL)) OR a0.c2)) THEN x'00' ELSE x'00' END) ELSE 1.5 END)) FROM t0 AS a0;
```

## What differed

```
an equivalent form gives other rows
  expected: 1 rows
    x'00'
  Amphora:  1 rows
    1.5
```
