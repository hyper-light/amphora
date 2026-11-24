# diff Wrong: Amphora's answer isn't SQLite's

seed 8005, state 23228

## Statements

```sql
CREATE TABLE t0(c0 NUMERIC PRIMARY KEY NOT NULL);
INSERT OR REPLACE INTO t0 VALUES (-100);
```

## Queries

```sql
SELECT json_pretty('{"a":null}', ('é' * '1e3')) FROM t0 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    '{
0.0"a": null
}'
  Amphora:  1 rows
    '{
0"a": null
}'
```
