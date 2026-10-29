# pqs Error: Amphora errors where SQLite finds the pivot: JSON cannot hold BLOB values

seed 20261027, state 17389

## Statements

```sql
CREATE TABLE t0(c0 REAL);
INSERT OR IGNORE INTO t0 VALUES (x''), (NULL);
INSERT OR IGNORE INTO t0 VALUES ('0x10');
INSERT INTO t0 VALUES ('-1');
```

## Queries

```sql
SELECT (('10' NOT BETWEEN (SELECT max(s927.c0) FROM t0 AS s927 WHERE (s927.c0 NOT LIKE '*')) AND json_array(NULL, a0.c0))) IS TRUE, (('10' NOT BETWEEN (SELECT max(s927.c0) FROM t0 AS s927 WHERE (s927.c0 NOT LIKE '*')) AND json_array(NULL, a0.c0))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 4;
SELECT a0.rowid FROM t0 AS a0 WHERE ('10' NOT BETWEEN (SELECT max(s927.c0) FROM t0 AS s927 WHERE (s927.c0 NOT LIKE '*')) AND json_array(NULL, a0.c0));
```

## What differed

```
Amphora errors where SQLite finds the pivot: JSON cannot hold BLOB values
```
