# diff Wrong: Amphora's answer isn't SQLite's

seed 27001, state 16204

## Statements

```sql
CREATE TABLE t2(c0 INTEGER);
INSERT OR REPLACE INTO t2 VALUES (1);
```

## Queries

```sql
SELECT jsonb_object('_', asinh(1)) FROM t2 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    x'cc17175fc513302e3838313337333538373031393534333035'
  Amphora:  1 rows
    x'cc15175fc511302e383831333733353837303139353433'
```
