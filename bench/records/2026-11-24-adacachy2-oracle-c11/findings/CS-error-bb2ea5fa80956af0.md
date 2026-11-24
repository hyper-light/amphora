# eet Error: the equivalent form errors on Amphora, not on SQLite: Parse error: Scalar subquery is not supported in this context

seed 8003, state 2

## Statements

```sql
CREATE TABLE t2(c0  COLLATE NOCASE PRIMARY KEY) WITHOUT ROWID;
CREATE TABLE lg(c0, c1, c2);
```

## Queries

```sql
SELECT min((a0.c0 IS NOT x'0102')) OVER (RANGE BETWEEN CURRENT ROW AND CURRENT ROW) FROM t2 AS a0;
SELECT min((a0.c0 IS NOT (CASE WHEN (((SELECT sum(s942.c2) FROM lg AS s942 WHERE NULL) AND (NOT (SELECT sum(s942.c2) FROM lg AS s942 WHERE NULL))) AND ((SELECT sum(s942.c2) FROM lg AS s942 WHERE NULL) NOTNULL)) THEN '-1' ELSE x'0102' END))) OVER (RANGE BETWEEN CURRENT ROW AND CURRENT ROW) FROM t2 AS a0;
```

## What differed

```
the equivalent form errors on Amphora, not on SQLite: Parse error: Scalar subquery is not supported in this context
```
