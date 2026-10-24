# eet Wrong: an equivalent form gives other rows

seed 20261024, state 595

## Statements

```sql
CREATE TABLE t0(c0 NUMERIC, c1 INTEGER, c2  UNIQUE, c3 DOUBLE PRIMARY KEY);
CREATE TABLE t1(c0 INTEGER PRIMARY KEY, c1 DECIMAL(10,2), c2 INTEGER COLLATE NOCASE UNIQUE, c3 BLOB);
INSERT OR IGNORE INTO t1 VALUES (9007199254740993, 'x', x'41', 0.5);
```

## Queries

```sql
SELECT NULL FROM t1 AS a0, t1 AS a1 LEFT JOIN t0 AS a2 ON (1 IN ((1e308 COLLATE NOCASE), a0.c3)) WHERE (((a2.c1 NOTNULL) AND (a1.c0 IS a1.c1)) OR (json_extract('{"a":null}', '$') < a1.c2));
SELECT NULL FROM t1 AS a0, t1 AS a1 LEFT JOIN t0 AS a2 ON (1 IN ((1e308 COLLATE NOCASE), a0.c3)) WHERE ((((NOT a2.c1) OR (NOT (NOT a2.c1))) OR ((NOT a2.c1) ISNULL)) AND (((a2.c1 NOTNULL) AND (CASE WHEN (((a0.c2 IS DISTINCT FROM 'abc') OR (NOT (a0.c2 IS DISTINCT FROM 'abc'))) OR ((a0.c2 IS DISTINCT FROM 'abc') ISNULL)) THEN (a1.c0 IS a1.c1) ELSE 255 END)) OR (json_extract('{"a":null}', '$') < a1.c2)));
```

## What differed

```
an equivalent form gives other rows
  expected: 1 rows
    NULL
  Amphora:  0 rows
```
