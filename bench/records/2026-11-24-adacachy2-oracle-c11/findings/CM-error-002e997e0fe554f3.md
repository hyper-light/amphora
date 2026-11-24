# norec Error: the unoptimized form errors on Amphora, not on SQLite: Parse error: insufficient registers allocated for expression vector write (start=4, count=2, next_free=5)

seed 8003, state 38

## Statements

```sql
CREATE TABLE t1(c0 DECIMAL(10,2) UNIQUE, c1 DOUBLE PRIMARY KEY, c2 TEXT, c3 REAL COLLATE BINARY NOT NULL);
```

## Queries

```sql
SELECT 1 FROM t1 AS a0, t1 AS a1 WHERE ((json_extract('{"a":null}', '$.a'), a0.c2) BETWEEN (json_extract('{"a":null}', '$.a'), (CAST(a1.c2 AS REAL) + json_extract('[1,2,3]', '$'))) AND (a1.c2, NULL));
SELECT (CASE WHEN ((json_extract('{"a":null}', '$.a'), a0.c2) BETWEEN (json_extract('{"a":null}', '$.a'), (CAST(a1.c2 AS REAL) + json_extract('[1,2,3]', '$'))) AND (a1.c2, NULL)) THEN 1 ELSE 0 END) FROM t1 AS a0, t1 AS a1;
```

## What differed

```
the unoptimized form errors on Amphora, not on SQLite: Parse error: insufficient registers allocated for expression vector write (start=4, count=2, next_free=5)
```
