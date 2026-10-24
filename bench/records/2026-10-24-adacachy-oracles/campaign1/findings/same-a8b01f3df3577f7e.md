# same Wrong: the statements leave t2 with other rows

seed 11, state 67

## Statements

```sql
CREATE TABLE t2(c0 REAL UNIQUE NOT NULL, c1 VARCHAR(10), c2 NUMERIC UNIQUE, c3 TEXT COLLATE RTRIM UNIQUE);
INSERT INTO t2 VALUES ('a', 1000000000000000.0, 'a', '%');
```

## Queries

```sql
SELECT rowid, * FROM t2;
```

## What differed

```
the statements leave t2 with other rows
  expected: 1 rows
    1, 'a', '1000000000000000.0', 'a', '%'
  Amphora:  1 rows
    1, 'a', '1.0e+15', 'a', '%'
```
