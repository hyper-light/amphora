# diff Wrong: window_subquery_arg

plain

## Statements

```sql
CREATE TABLE t(a INT);
CREATE TABLE s(b INT);
INSERT INTO t VALUES (1), (2);
INSERT INTO s VALUES (2);
```

## Queries

```sql
SELECT a, sum(CASE WHEN a IN (SELECT b FROM s) THEN 1 ELSE 0 END) OVER (ORDER BY a) FROM t;
SELECT a, sum((SELECT max(b) FROM s)) OVER (ORDER BY a) FROM t;
```

## What differed

```
```
