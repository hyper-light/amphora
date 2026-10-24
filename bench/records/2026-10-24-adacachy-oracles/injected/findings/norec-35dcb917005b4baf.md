# norec Wrong: WHERE p keeps 0 rows; p is TRUE for 1 of them

seed 20261024, state 964

## Statements

```sql
CREATE TABLE t0(c0 , c1 TEXT, c2 INTEGER COLLATE NOCASE PRIMARY KEY);
INSERT OR IGNORE INTO t0 VALUES (3.0, 1000000000000000.0, '10');
```

## Queries

```sql
SELECT 1 FROM t0 AS a0, t0 AS a1 RIGHT JOIN t0 AS a2 ON ((NOT (a1.c1 <> 255)) BETWEEN strftime(a1.c1, 0.5) AND json_extract('"x"', '$[0]')) WHERE (a0.c2 ISNULL);
SELECT (CASE WHEN (a0.c2 ISNULL) THEN 1 ELSE 0 END) FROM t0 AS a0, t0 AS a1 RIGHT JOIN t0 AS a2 ON ((NOT (a1.c1 <> 255)) BETWEEN strftime(a1.c1, 0.5) AND json_extract('"x"', '$[0]'));
```

## What differed

```
WHERE p keeps 0 rows; p is TRUE for 1 of them
```
