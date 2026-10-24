# tlp Error: the partitions' aggregate errors on Amphora, not on SQLite: Parse error: no such table: a2

seed 14, state 1194

## Statements

```sql
CREATE TABLE t2(c0 INTEGER UNIQUE);
```

## Queries

```sql
SELECT count(NULL) FROM t2 AS a0 WHERE 0;
SELECT sum(m) FROM (SELECT NULL AS m WHERE 0 UNION ALL SELECT count(NULL) FROM t2 AS a0 WHERE (0 AND a2.c0) UNION ALL SELECT count(NULL) FROM t2 AS a0 WHERE (0 AND (NOT a2.c0)) UNION ALL SELECT count(NULL) FROM t2 AS a0 WHERE (0 AND (a2.c0 ISNULL)));
```

## What differed

```
the partitions' aggregate errors on Amphora, not on SQLite: Parse error: no such table: a2
```
