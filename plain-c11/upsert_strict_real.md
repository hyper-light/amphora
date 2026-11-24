# diff Wrong: upsert_strict_real

plain

## Statements

```sql
CREATE TABLE m(k TEXT PRIMARY KEY, v REAL) STRICT;
INSERT INTO m VALUES ('a', 1);
INSERT INTO m VALUES ('a', 2) ON CONFLICT(k) DO UPDATE SET v = excluded.v;
```

## Queries

```sql
SELECT k, v, typeof(v) FROM m;
```

## What differed

```
```
