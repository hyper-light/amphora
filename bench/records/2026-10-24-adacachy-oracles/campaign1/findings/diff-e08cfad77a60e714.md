# diff Wrong: Amphora's answer isn't SQLite's

seed 11, state 789

## Statements

```sql
CREATE TABLE t0(c0 );
INSERT INTO t0 VALUES ('');
UPDATE t0 SET c0 = 9007199254740993 WHERE c0 IS NOT NULL;
```

## Queries

```sql
SELECT ltrim(a1.c0, a0.c0) FROM t0 AS a0, t0 AS a1 WHERE (CAST(json_extract('{"a":null}', '$.b[1]') AS BLOB) IS NOT DISTINCT FROM (CASE WHEN (a1.c0 ISNULL) THEN (+ ' ') ELSE unhex(a0.c0, a0.c0) END));
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 0 rows
  Amphora:  1 rows
    ''
```
