# pqs Error: Amphora errors where SQLite finds the pivot: JSON cannot hold BLOB values

seed 12, state 823

## Statements

```sql
CREATE TABLE t0(c0 INT UNIQUE, c1 );
INSERT INTO t0 VALUES (9223372036854775807, x'41'), (x'00', -2.5), ('true', 'A');
INSERT OR IGNORE INTO t0 VALUES (NULL, -9223372036854775808);
INSERT OR IGNORE INTO t0 VALUES (-9223372036854775808, 7), (0.0, 3.0), (-100, 'x');
```

## Queries

```sql
SELECT ((x'00' AND jsonb_array(NULL, a0.c1))) IS TRUE, ((x'00' AND jsonb_array(NULL, a0.c1))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 7;
SELECT a0.rowid FROM t0 AS a0 WHERE (NOT (x'00' AND jsonb_array(NULL, a0.c1)));
```

## What differed

```
Amphora errors where SQLite finds the pivot: JSON cannot hold BLOB values
```
