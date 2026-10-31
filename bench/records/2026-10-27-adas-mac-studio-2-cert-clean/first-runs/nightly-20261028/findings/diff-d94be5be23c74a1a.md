# diff Wrong: Amphora's answer isn't SQLite's

seed 20261028, state 60143

## Statements

```sql
CREATE TABLE t0(c0 BLOB COLLATE NOCASE PRIMARY KEY, c1 TEXT, c2 INTEGER UNIQUE);
INSERT OR IGNORE INTO t0 VALUES (x'00', 9007199254740993, 10);
```

## Queries

```sql
SELECT unicode(unhex(a0.c1)) FROM t0 AS a0;
```

## What differed

```
Amphora's answer isn't SQLite's
  expected: 1 rows
    144
  Amphora:  1 rows
    65533
```
