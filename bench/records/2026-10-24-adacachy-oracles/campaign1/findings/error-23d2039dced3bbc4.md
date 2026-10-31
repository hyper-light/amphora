# tlp Error: the partitions' aggregate errors on Amphora, not on SQLite: FLAGS parameter to json_valid() must be between 1 and 15

seed 11, state 77

## Statements

```sql
CREATE TABLE t0(c0 INTEGER NOT NULL, c1 DECIMAL(10,2), c2  COLLATE NOCASE);
INSERT OR IGNORE INTO t0 VALUES ('1', NULL, NULL);
CREATE TABLE t1(c0 DECIMAL(10,2) UNIQUE NOT NULL, c1 TEXT COLLATE NOCASE, c2  COLLATE BINARY);
```

## Queries

```sql
SELECT min(((rtrim(-2.5) || (CASE a2.c0 WHEN (x'0102' IN (a1.c2)) THEN a2.c1 WHEN (x'ff' <> a1.c2) THEN a0.c0 ELSE a1.c2 END)) COLLATE BINARY)) FROM t0 AS a0, t1 AS a1 FULL JOIN t1 AS a2 ON (NOT (a1.c2 IS NOT DISTINCT FROM a2.c0)) WHERE (a1.c1 NOTNULL);
SELECT min(m COLLATE BINARY) FROM (SELECT NULL AS m WHERE 0 UNION ALL SELECT min(((rtrim(-2.5) || (CASE a2.c0 WHEN (x'0102' IN (a1.c2)) THEN a2.c1 WHEN (x'ff' <> a1.c2) THEN a0.c0 ELSE a1.c2 END)) COLLATE BINARY)) FROM t0 AS a0, t1 AS a1 FULL JOIN t1 AS a2 ON (NOT (a1.c2 IS NOT DISTINCT FROM a2.c0)) WHERE ((a1.c1 NOTNULL) AND (a1.c1 > (- json_valid(x'41', a0.c2)))) UNION ALL SELECT min(((rtrim(-2.5) || (CASE a2.c0 WHEN (x'0102' IN (a1.c2)) THEN a2.c1 WHEN (x'ff' <> a1.c2) THEN a0.c0 ELSE a1.c2 END)) COLLATE BINARY)) FROM t0 AS a0, t1 AS a1 FULL JOIN t1 AS a2 ON (NOT (a1.c2 IS NOT DISTINCT FROM a2.c0)) WHERE ((a1.c1 NOTNULL) AND (NOT (a1.c1 > (- json_valid(x'41', a0.c2))))) UNION ALL SELECT min(((rtrim(-2.5) || (CASE a2.c0 WHEN (x'0102' IN (a1.c2)) THEN a2.c1 WHEN (x'ff' <> a1.c2) THEN a0.c0 ELSE a1.c2 END)) COLLATE BINARY)) FROM t0 AS a0, t1 AS a1 FULL JOIN t1 AS a2 ON (NOT (a1.c2 IS NOT DISTINCT FROM a2.c0)) WHERE ((a1.c1 NOTNULL) AND ((a1.c1 > (- json_valid(x'41', a0.c2))) ISNULL)));
```

## What differed

```
the partitions' aggregate errors on Amphora, not on SQLite: FLAGS parameter to json_valid() must be between 1 and 15
```
