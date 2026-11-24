# norec Error: the unoptimized form errors on Amphora, not on SQLite: Internal error: secondary index lacks primary key locator

seed 8003, state 93

## Statements

```sql
CREATE TABLE t0(c0 DECIMAL(10,2) COLLATE NOCASE, c1 , c2 INT, c3 INT, PRIMARY KEY(c0, c1)) WITHOUT ROWID;
CREATE TABLE lg(c0, c1, c2);
CREATE INDEX i2 ON t0(c2 COLLATE NOCASE DESC, c3 COLLATE RTRIM);
```

## Queries

```sql
SELECT 1 FROM t0 AS a0, lg AS a1 RIGHT JOIN t0 AS a2 ON (json_quote(a2.c2) NOT LIKE '_') WHERE (a1.c1 >= ((CASE WHEN ('A' NOTNULL) THEN a0.c0 ELSE a1.c1 END) || CAST('1.5' AS REAL)));
SELECT (CASE WHEN (a1.c1 >= ((CASE WHEN ('A' NOTNULL) THEN a0.c0 ELSE a1.c1 END) || CAST('1.5' AS REAL))) THEN 1 ELSE 0 END) FROM t0 AS a0, lg AS a1 RIGHT JOIN t0 AS a2 ON (json_quote(a2.c2) NOT LIKE '_');
```

## What differed

```
the unoptimized form errors on Amphora, not on SQLite: Internal error: secondary index lacks primary key locator
```
