# pqs Error: Amphora errors where SQLite finds the pivot: json_object() requires an even number of arguments

seed 13, state 4779

## Statements

```sql
CREATE TABLE t0(c0 DECIMAL(10,2));
INSERT INTO t0 VALUES ('é'), ('_'), (2);
INSERT INTO t0 VALUES (3.0), (100);
```

## Queries

```sql
SELECT (((CASE 1 WHEN 1 THEN json_object(NULL, 0.0, 3) WHEN (NULL IN (SELECT s588.c0 FROM t0 AS s588 WHERE -100)) THEN NULL ELSE NULL END) <= NULL)) IS TRUE, (((CASE 1 WHEN 1 THEN json_object(NULL, 0.0, 3) WHEN (NULL IN (SELECT s588.c0 FROM t0 AS s588 WHERE -100)) THEN NULL ELSE NULL END) <= NULL)) IS FALSE FROM t0 AS a0 WHERE a0.rowid = 5;
SELECT a0.rowid FROM t0 AS a0 WHERE (((CASE 1 WHEN 1 THEN json_object(NULL, 0.0, 3) WHEN (NULL IN (SELECT s588.c0 FROM t0 AS s588 WHERE -100)) THEN NULL ELSE NULL END) <= NULL) ISNULL);
```

## What differed

```
Amphora errors where SQLite finds the pivot: json_object() requires an even number of arguments
```
