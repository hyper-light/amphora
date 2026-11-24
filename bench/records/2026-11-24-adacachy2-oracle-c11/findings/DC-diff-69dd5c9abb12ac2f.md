# diff Panic: cursor id 4 is None

seed 8002, state 12891

## Statements

```sql
CREATE TABLE t0(c0 INTEGER COLLATE NOCASE NOT NULL, c1 BLOB UNIQUE, c2 ANY, c3 ANY COLLATE BINARY PRIMARY KEY) STRICT;
CREATE TABLE t1(c0 REAL COLLATE NOCASE PRIMARY KEY, c1 INTEGER, c2 INTEGER, c3 NUMERIC);
INSERT INTO t0 VALUES (2, x'c3bf', x'c3bf', 'x');
INSERT OR IGNORE INTO t1 VALUES ('é', 0, 1, 7);
```

## Queries

```sql
SELECT '"x"' FROM t0 AS a0 JOIN t1 AS a1 ON (CASE WHEN (a0.c1 <= NULL) THEN NULL END) JOIN t0 AS a2 ON ((a0.c0, NULL, a2.c2) BETWEEN (1000000000000000.0, 0.1, a0.c0) AND (7, '10', x'41')) WHERE (a2.c2 >= 100);
```

## What differed

```
cursor id 4 is None
```
