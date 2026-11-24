# diff Wrong: Amphora's answer isn't SQLite's

seed 8002, state 4563

## Statements

```sql
CREATE TABLE t0(c0 INTEGER COLLATE NOCASE PRIMARY KEY);
INSERT INTO t0 VALUES (2);
```

## Queries

```sql
SELECT jsonb_array(exp(a0.c0)) FROM t0 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    x'cb14c512372e33383930353630393839333036353034'
  Amphora:  1 rows
    x'cb12c510372e3338393035363039383933303635'
```
