# norec Error: the unoptimized form errors on Amphora, not on SQLite: Parse error: insufficient registers allocated for expression vector write (start=4, count=3, next_free=5)

seed 27004, state 23

## Statements

```sql
CREATE TABLE t0(c0 INTEGER COLLATE NOCASE PRIMARY KEY, c1  UNIQUE NOT NULL);
CREATE TABLE lg(c0, c1, c2);
```

## Queries

```sql
SELECT 1 FROM lg AS a0, t0 AS a1 WHERE ((json_error_position((+ '0x10')), (a1.c1 = a1.c0), ('%' NOT IN (a0.c1, a1.c1, 0.1))) BETWEEN (((CASE WHEN (a0.c2 <= NULL) THEN 'true' WHEN (a0.c1 == a0.c0) THEN a1.c0 ELSE a1.c0 END) COLLATE BINARY), ((- a1.c0) - if(a0.c2, a0.c2)), json_extract('"x"', '$')) AND ((NOT ('1' NOTNULL)), (a0.c1 COLLATE RTRIM), a1.c0));
SELECT (CASE WHEN ((json_error_position((+ '0x10')), (a1.c1 = a1.c0), ('%' NOT IN (a0.c1, a1.c1, 0.1))) BETWEEN (((CASE WHEN (a0.c2 <= NULL) THEN 'true' WHEN (a0.c1 == a0.c0) THEN a1.c0 ELSE a1.c0 END) COLLATE BINARY), ((- a1.c0) - if(a0.c2, a0.c2)), json_extract('"x"', '$')) AND ((NOT ('1' NOTNULL)), (a0.c1 COLLATE RTRIM), a1.c0)) THEN 1 ELSE 0 END) FROM lg AS a0, t0 AS a1;
```

## What differed

```
the unoptimized form errors on Amphora, not on SQLite: Parse error: insufficient registers allocated for expression vector write (start=4, count=3, next_free=5)
```
