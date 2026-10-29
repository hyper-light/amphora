# pqs Error: Amphora errors where SQLite finds the pivot: Parse error: Unexpected end of input

seed 20261026, state 82002

## Statements

```sql
CREATE TABLE t0(c0 DOUBLE COLLATE BINARY, c1 DOUBLE COLLATE NOCASE PRIMARY KEY, c2 DOUBLE);
INSERT OR IGNORE INTO t0 VALUES ('10', 100, x''), ('1', -100, -9223372036854775808);
```

## Queries

```sql
SELECT ((SELECT min(s603.c1) FROM t0 AS s603 WHERE (json(s603.c2) NOTNULL))) IS TRUE, ((SELECT min(s603.c1) FROM t0 AS s603 WHERE (json(s603.c2) NOTNULL))) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 1;
SELECT a0.rowid FROM t0 AS a0 WHERE (SELECT min(s603.c1) FROM t0 AS s603 WHERE (json(s603.c2) NOTNULL));
```

## What differed

```
Amphora errors where SQLite finds the pivot: Parse error: Unexpected end of input
```
