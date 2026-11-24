# diff Wrong: Amphora's answer isn't SQLite's

seed 27010, state 9321

## Statements

```sql
CREATE TABLE t1(c0  COLLATE RTRIM PRIMARY KEY, c1 VARCHAR(10) UNIQUE NOT NULL);
INSERT OR IGNORE INTO t1 VALUES (7, 10);
```

## Queries

```sql
SELECT json_pretty('{"a":null}', 1000000000000000.0) FROM (SELECT NULL AS c0, NULL AS c1 FROM t1 AS b16x0 GROUP BY NULL) AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    '{
1000000000000000.0"a": null
}'
  Amphora:  1 rows
    '{
1000000000000000"a": null
}'
```
